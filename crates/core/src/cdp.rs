//! The DevTools protocol, as both hosts speak it.
//!
//! The window drives an embedded WebView2 and the server drives a headless
//! Chromium, and underneath they are the same browser answering the same
//! protocol. What differs is how a message gets there -- a COM call on one
//! side, a WebSocket frame on the other. What does not differ is the message
//! itself, so it is written once, here.
//!
//! Nothing in this module talks to a browser. It turns what a person did into
//! what the protocol calls it, which is why it can be read and tested without
//! one.

/// What a key name means to the protocol: the `key` a page sees, and the
/// virtual-key code that goes with it.
///
/// The names come from [`shikisha_shared::NAMED_KEYS`]; a name there with no
/// entry here would be accepted, dispatched, and do nothing at all -- which is
/// the one failure a person cannot diagnose. The test below is what stops that.
pub fn named_vk(named: &str) -> Option<(&'static str, u32)> {
    Some(match named {
        "enter" => ("Enter", 13),
        "backspace" => ("Backspace", 8),
        "tab" => ("Tab", 9),
        "escape" | "esc" => ("Escape", 27),
        "delete" => ("Delete", 46),
        "up" => ("ArrowUp", 38),
        "down" => ("ArrowDown", 40),
        "left" => ("ArrowLeft", 37),
        "right" => ("ArrowRight", 39),
        "space" => (" ", 32),
        "home" => ("Home", 36),
        "end" => ("End", 35),
        "pageup" => ("PageUp", 33),
        "pagedown" => ("PageDown", 34),
        "f1" => ("F1", 112),
        "f2" => ("F2", 113),
        "f3" => ("F3", 114),
        "f4" => ("F4", 115),
        "f5" => ("F5", 116),
        "f6" => ("F6", 117),
        "f7" => ("F7", 118),
        "f8" => ("F8", 119),
        "f9" => ("F9", 120),
        "f10" => ("F10", 121),
        "f11" => ("F11", 122),
        "f12" => ("F12", 123),
        _ => return None,
    })
}

/// The down and up that one named key press is made of.
///
/// Empty for a name the protocol has no key for, so a caller that dispatches
/// everything this returns sends nothing rather than something wrong.
pub fn key_events(named: &str, ctrl: bool, alt: bool) -> Vec<serde_json::Value> {
    let Some((key, vk)) = named_vk(named) else {
        return Vec::new();
    };
    // The protocol's modifier bits: Alt=1, Ctrl=2, Meta=4, Shift=8
    let mods = u32::from(alt) | (u32::from(ctrl) << 1);
    ["keyDown", "keyUp"]
        .into_iter()
        .map(|kind| {
            let mut ev = serde_json::json!({
                "type": kind,
                "key": key,
                "windowsVirtualKeyCode": vk,
                "nativeVirtualKeyCode": vk,
                "modifiers": mods,
            });
            // Space needs its text attached or it does not land in the field.
            // Held with a modifier it is a shortcut, and a shortcut types
            // nothing
            if kind == "keyDown" && named == "space" && mods == 0 {
                ev["text"] = serde_json::Value::from(" ");
            }
            ev
        })
        .collect()
}

/// Typing a string that is already decided.
///
/// One key event per character rather than `Input.insertText`: some sites
/// ignore the input event that insertText produces, and a character key event
/// is indistinguishable from a person's. Whatever conversion an IME had to do
/// happened on the sender's side, long before this.
pub fn text_events(text: &str) -> Vec<serde_json::Value> {
    text.chars()
        .map(|ch| {
            let mut buf = [0u8; 4];
            serde_json::json!({ "type": "char", "text": ch.encode_utf8(&mut buf) })
        })
        .collect()
}

/// One mouse event, and whether the button is still held after it.
///
/// The held flag is carried by the caller because a drag is a chain of moves
/// between a press and a release, and a move has to say whether it is part of
/// one.
pub fn mouse_event(
    phase: &str,
    x: f64,
    y: f64,
    down: bool,
    held: bool,
) -> (serde_json::Value, bool) {
    let (kind, buttons, now) = match phase {
        "pressed" => ("mousePressed", 1, true),
        "released" => ("mouseReleased", 0, false),
        _ => ("mouseMoved", i32::from(down || held), held),
    };
    (
        serde_json::json!({
            "type": kind, "x": x, "y": y,
            "button": "left", "buttons": buttons, "clickCount": 1,
        }),
        now,
    )
}

pub fn wheel_event(x: f64, y: f64, dx: f64, dy: f64) -> serde_json::Value {
    serde_json::json!({
        "type": "mouseWheel", "x": x, "y": y, "deltaX": dx, "deltaY": dy,
    })
}

/// Re-shape a page's viewport to the shape of the screen looking at it.
///
/// A phone in portrait looking at a page shaped for a desktop sees a strip
/// across the middle with black above and below it. The width is kept -- the
/// page is still a desktop page, and a site that switches to its mobile layout
/// below 400px would become a different site -- and the height is stretched to
/// the viewer's aspect ratio.
///
/// `None` means take the override away: the page's own shape is already as
/// tall as the viewer wants, which is what a phone turned sideways reports.
pub fn view_metrics(nat: (f64, f64), w: f64, h: f64) -> Option<serde_json::Value> {
    if nat.0 < 1.0 || nat.1 < 1.0 || w <= 0.0 || h <= 0.0 {
        return None;
    }
    let want_h = (nat.0 * (h / w).clamp(0.2, 3.0)).round();
    (want_h > nat.1 * 1.02).then(|| {
        serde_json::json!({
            "width": nat.0.round(),
            "height": want_h,
            "deviceScaleFactor": 0,
            "mobile": false,
        })
    })
}

/// What a screencast is asked for.
///
/// The height allows for a portrait viewer, so a tall frame is sent at its own
/// size instead of being scaled down and arriving blurred.
pub const CAST_PARAMS: &str =
    "{\"format\":\"jpeg\",\"quality\":60,\"maxWidth\":1600,\"maxHeight\":2400,\"everyNthFrame\":1}";

/// The centre of the first quad with any area in it, in viewport pixels.
///
/// An element can report several boxes -- a link broken across two lines -- and
/// one of them can be collapsed to nothing. The middle of a zero-area box is
/// whatever is behind it.
pub fn quad_center(q: &serde_json::Value) -> Option<(f64, f64)> {
    for quad in q.get("quads")?.as_array()? {
        let p: Vec<f64> = quad
            .as_array()?
            .iter()
            .filter_map(serde_json::Value::as_f64)
            .collect();
        if p.len() == 8 {
            let x = (p[0] + p[2] + p[4] + p[6]) / 4.0;
            let y = (p[1] + p[3] + p[5] + p[7]) / 4.0;
            // A zero-area quad is a collapsed (invisible) box
            if (p[0] - p[2]).abs() + (p[1] - p[7]).abs() > 0.5 {
                return Some((x, y));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every key name the vocabulary offers is one a host knows how to press.
    ///
    /// A name is turned away up in the runtime against the shared list and
    /// nothing else -- it cannot see this table. A name on that list with no
    /// entry here would be accepted, dispatched, and do nothing
    #[test]
    fn every_named_key_has_something_to_press() {
        for named in shikisha_shared::NAMED_KEYS {
            assert!(named_vk(named).is_some(), "押し方の分からないキー名: {named}");
        }
    }

    #[test]
    fn a_key_press_is_a_down_and_an_up() {
        let evs = key_events("enter", false, false);
        assert_eq!(evs.len(), 2);
        assert_eq!(evs[0]["type"], "keyDown");
        assert_eq!(evs[1]["type"], "keyUp");
        assert_eq!(evs[0]["key"], "Enter");
        assert_eq!(evs[0]["modifiers"], 0);
        assert!(
            key_events("kaboom", false, false).is_empty(),
            "知らない名前を押している"
        );
    }

    /// Ctrl and Alt are bits, and the pair of them is both bits
    #[test]
    fn modifiers_are_the_bits_the_protocol_names() {
        assert_eq!(key_events("f1", true, false)[0]["modifiers"], 2);
        assert_eq!(key_events("f1", false, true)[0]["modifiers"], 1);
        assert_eq!(key_events("f1", true, true)[0]["modifiers"], 3);
    }

    /// A bare space types a space; a held space is a shortcut and types nothing
    #[test]
    fn space_carries_its_own_text_unless_it_is_a_shortcut() {
        assert_eq!(key_events("space", false, false)[0]["text"], " ");
        assert!(key_events("space", true, false)[0].get("text").is_none());
        assert!(
            key_events("space", false, false)[1].get("text").is_none(),
            "離す方に文字が付いている"
        );
    }

    /// Typing is counted in characters, not bytes
    #[test]
    fn typing_sends_one_event_per_character() {
        let evs = text_events("あa");
        assert_eq!(evs.len(), 2, "文字数で分けていない: {evs:?}");
        assert_eq!(evs[0]["text"], "あ");
        assert_eq!(evs[1]["text"], "a");
    }

    /// A move between a press and a release is part of the drag
    #[test]
    fn a_drag_is_a_press_some_moves_and_a_release() {
        let (down, held) = mouse_event("pressed", 10.0, 20.0, false, false);
        assert_eq!(down["type"], "mousePressed");
        assert!(held, "押したのに離れている");
        let (moved, held) = mouse_event("moved", 30.0, 20.0, false, held);
        assert_eq!(moved["buttons"], 1, "ドラッグ中の移動がボタン無しになっている");
        assert!(held);
        let (up, held) = mouse_event("released", 30.0, 20.0, false, held);
        assert_eq!(up["type"], "mouseReleased");
        assert!(!held, "離したのに押したままになっている");
        // A move with nothing held is a hover
        assert_eq!(mouse_event("moved", 1.0, 1.0, false, held).0["buttons"], 0);
    }

    /// A portrait viewer gets a taller page; a landscape one gets its own back
    #[test]
    fn the_viewport_follows_the_shape_of_the_screen_looking_at_it() {
        let nat = (1200.0, 800.0);
        let tall = view_metrics(nat, 400.0, 900.0).expect("縦長の画面に合わせていない");
        assert_eq!(tall["width"], 1200.0, "幅まで変えている");
        assert_eq!(tall["height"], 2700.0);
        assert!(view_metrics(nat, 900.0, 400.0).is_none(), "横長で上書きしている");
        // Nothing to go on: the page has not produced a frame yet
        assert!(view_metrics((0.0, 0.0), 400.0, 900.0).is_none());
    }

    /// The first box with any area in it, not simply the first box
    #[test]
    fn a_collapsed_box_is_not_where_a_click_goes() {
        let q = serde_json::json!({ "quads": [
            [10.0, 10.0, 10.0, 10.0, 10.0, 10.0, 10.0, 10.0],
            [0.0, 0.0, 100.0, 0.0, 100.0, 50.0, 0.0, 50.0],
        ]});
        assert_eq!(quad_center(&q), Some((50.0, 25.0)));
        assert_eq!(quad_center(&serde_json::json!({ "quads": [] })), None);
    }
}

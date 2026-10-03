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
///
/// `clicks` is how many presses in a row this one is, as counted where the
/// person pressed. A page is only told a double click happened when the press
/// and the release both say two, so the number travels the whole way rather
/// than being decided here. Nothing said means one.
pub fn mouse_event(
    phase: &str,
    x: f64,
    y: f64,
    down: bool,
    held: bool,
    clicks: u8,
) -> (serde_json::Value, bool) {
    let (kind, buttons, now) = match phase {
        "pressed" => ("mousePressed", 1, true),
        "released" => ("mouseReleased", 0, false),
        _ => ("mouseMoved", i32::from(down || held), held),
    };
    (
        serde_json::json!({
            "type": kind, "x": x, "y": y,
            "button": "left", "buttons": buttons, "clickCount": clicks.clamp(1, 3),
        }),
        now,
    )
}

pub fn wheel_event(x: f64, y: f64, dx: f64, dy: f64) -> serde_json::Value {
    serde_json::json!({
        "type": "mouseWheel", "x": x, "y": y, "deltaX": dx, "deltaY": dy,
    })
}

/// Lay out a relayed page at the viewer's CSS width and height. Ordinary
/// pages and DevTools follow the same rule: retaining the desktop width
/// shrinks the lettering and prevents responsive layouts from reflowing.
///
/// `zoom` is the browser zoom the caller puts on that screen (1 where it has
/// none to put). Its viewport is the viewer's screen times that, so at the
/// viewer's density ([`viewer_zoom`]) it is laid out at the viewer's width
/// with a pixel for every pixel shown. A picture is only ever as many pixels
/// as the viewport is wide -- a screencast does not follow an emulated
/// density (tried: `deviceScaleFactor` and `scale` both left the picture at
/// 390 across) -- so the pixels have to be in the viewport, and the zoom is
/// what keeps the layout at the viewer's width. It has to be the browser's
/// own: a CSS zoom on the page left the DevTools measuring its tabs in one
/// scale and drawing them in the other (the tabs that did not fit were drawn
/// over the panel). At one to one it is the right size and stretched at the
/// far end. No source frame is needed: the viewer supplies both dimensions.
/// `None` means the viewer has no usable area yet.
pub fn view_metrics(w: f64, h: f64, zoom: f64) -> Option<serde_json::Value> {
    if !w.is_finite() || !h.is_finite() || !zoom.is_finite() || w < 1.0 || h < 1.0 {
        return None;
    }
    let z = zoom.max(1.0);
    Some(serde_json::json!({
        "width": (w * z).floor(),
        "height": (h * z).floor(),
        "deviceScaleFactor": 0,
        "mobile": false,
    }))
}

/// How much a screen fitted to its viewer is zoomed: the viewer's density,
/// held so the picture stays inside what a screencast is let be
/// ([`CAST_LONGEST`]) and never below one (a screen with no more pixels than
/// CSS pixels is shown as it is). To two places and down, not to the nearest:
/// up could take the picture past its limit
pub fn viewer_zoom(w: f64, h: f64, dpr: f64) -> f64 {
    let most = CAST_LONGEST / w.max(h).max(1.0);
    let z = if dpr > 0.0 { dpr } else { 1.0 }.min(most).max(1.0);
    (z * 100.0).floor() / 100.0
}


/// The longest side a screencast picture may have, in pixels. Chosen to hold
/// a portrait phone's whole screen at its own density -- the tallest in common
/// use are 2,532 to 2,556 pixels -- so the picture a phone gets is not shrunk
/// on the way and stretched back at the end. Larger ones are brought down to it
pub const CAST_LONGEST: f64 = 2560.0;

/// What a screencast is asked for.
///
/// Room for [`CAST_LONGEST`] either way, so a tall frame is sent at its own
/// size instead of being scaled down and arriving blurred.
pub const CAST_PARAMS: &str =
    "{\"format\":\"jpeg\",\"quality\":60,\"maxWidth\":2560,\"maxHeight\":2560,\"everyNthFrame\":1}";

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
            assert!(named_vk(named).is_some(), "a key name it does not know how to press: {named}");
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
            "it presses a name it does not know"
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
            "the release carries a character"
        );
    }

    /// Typing is counted in characters, not bytes
    #[test]
    fn typing_sends_one_event_per_character() {
        let evs = text_events("あa");
        assert_eq!(evs.len(), 2, "it does not split by character count: {evs:?}");
        assert_eq!(evs[0]["text"], "あ");
        assert_eq!(evs[1]["text"], "a");
    }

    /// A move between a press and a release is part of the drag
    #[test]
    fn a_drag_is_a_press_some_moves_and_a_release() {
        let (down, held) = mouse_event("pressed", 10.0, 20.0, false, false, 1);
        assert_eq!(down["type"], "mousePressed");
        assert!(held, "it was pressed but is released");
        let (moved, held) = mouse_event("moved", 30.0, 20.0, false, held, 1);
        assert_eq!(moved["buttons"], 1, "moving while dragging has no button");
        assert!(held);
        let (up, held) = mouse_event("released", 30.0, 20.0, false, held, 1);
        assert_eq!(up["type"], "mouseReleased");
        assert!(!held, "it was released but is still held");
        // A move with nothing held is a hover
        assert_eq!(mouse_event("moved", 1.0, 1.0, false, held, 1).0["buttons"], 0);
    }

    /// The second press of a double click says so, and so does its release.
    ///
    /// A page decides `dblclick` from the count on the events it is handed;
    /// two presses that both say "one" are two clicks, however fast they came
    #[test]
    fn a_double_click_arrives_as_a_second_press_that_says_it_is_the_second() {
        assert_eq!(mouse_event("pressed", 1.0, 1.0, false, false, 2).0["clickCount"], 2);
        assert_eq!(mouse_event("released", 1.0, 1.0, false, true, 2).0["clickCount"], 2);
        // Nothing said, and anything beyond a triple, are both a press
        assert_eq!(mouse_event("pressed", 1.0, 1.0, false, false, 0).0["clickCount"], 1);
        assert_eq!(mouse_event("pressed", 1.0, 1.0, false, false, 9).0["clickCount"], 3);
    }

    /// Both orientations use the viewer's width, even before a frame exists.
    #[test]
    fn the_viewport_follows_the_shape_of_the_screen_looking_at_it() {
        let tall = view_metrics(400.0, 900.0, 1.0).unwrap();
        assert_eq!(tall["width"], 400.0);
        assert_eq!(tall["height"], 900.0);
        let wide = view_metrics(900.0, 400.0, 1.0).unwrap();
        assert_eq!(wide["width"], 900.0);
        assert_eq!(wide["height"], 400.0);
    }

    /// A hidden viewer must not clear a page's usable shape or hand invalid
    /// numbers to the browser protocol.
    #[test]
    fn an_unmeasurable_viewer_does_not_supply_a_viewport() {
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(view_metrics(bad, 844.0, 1.0).is_none());
            assert!(view_metrics(390.0, bad, 1.0).is_none());
        }
        assert!(view_metrics(390.0, 844.0, f64::NAN).is_none());
    }

    /// A relayed page gets the phone's screen in the phone's pixels
    /// and is zoomed by its density, so it lays itself out at the phone's
    /// width with a pixel for every pixel shown: shrunk from a PC's width its
    /// lettering was a third of its size, and at one to one it was stretched
    #[test]
    fn a_relayed_page_takes_the_screen_looking_at_it_pixel_for_pixel() {
        let z = viewer_zoom(390.0, 718.0, 3.0);
        assert_eq!(z, 3.0);
        let m = view_metrics(390.0, 718.0, z).unwrap();
        assert_eq!((m["width"].as_f64(), m["height"].as_f64()), (Some(1170.0), Some(2154.0)));
        // With no zoom to put on it, the phone's own width, one to one
        let flat = view_metrics(390.0, 718.0, 1.0).unwrap();
        assert_eq!(flat["width"], 390.0);
        // Turned sideways it still takes the phone's shape
        let side = view_metrics(844.0, 390.0, viewer_zoom(844.0, 390.0, 3.0)).unwrap();
        assert_eq!(side["width"], 2532.0);
        // A density that would take the picture past its limit is brought down
        // to it, and a screen with no more pixels than CSS pixels is not zoomed
        let z = viewer_zoom(430.0, 932.0, 3.0);
        assert!(932.0 * z <= CAST_LONGEST && z < 3.0, "{z}");
        assert_eq!(viewer_zoom(390.0, 718.0, 1.0), 1.0);
        assert_eq!(viewer_zoom(390.0, 718.0, 0.5), 1.0);
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

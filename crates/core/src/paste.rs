//! This PC's clipboard, pasted into a tab.
//!
//! Here rather than in the window's crate because two different processes
//! paste: the window, when the program is one process, and the runtime, when
//! it is split in two and the window's right-click arrives over the board
//! (`remote::allowed_from_here`). One way of pasting, so the two cannot come to
//! paste differently.

use crate::tab::Tab;

/// Pastes the clipboard's text into `t`, as a paste when the program in it
/// asked to be told (bracketed paste), so a pasted line is not run on its own
/// Enter. `Some` is a sentence to show: what went wrong
#[cfg(windows)]
pub fn clipboard_into(t: &Tab) -> anyhow::Result<Option<String>> {
    let got = arboard::Clipboard::new().and_then(|mut c| c.get_text());
    // A picture, into a tab on another machine: an AI there cannot read this
    // PC's clipboard, as one here does. Sent up to the folder there as 📎
    // sends a file, and its path typed in its place
    if got.is_err()
        && let Some(said) = image_far(t)
    {
        return said;
    }
    match got {
        Ok(text) => {
            let bracketed = t.parser.lock().unwrap_or_else(|e| e.into_inner()).screen().bracketed_paste();
            let normalized = text.replace("\r\n", "\r").replace('\n', "\r");
            if bracketed {
                let mut bytes = b"\x1b[200~".to_vec();
                bytes.extend_from_slice(normalized.as_bytes());
                bytes.extend_from_slice(b"\x1b[201~");
                t.write_bytes(&bytes)?;
            } else {
                t.write_bytes(normalized.as_bytes())?;
            }
            Ok(None)
        }
        Err(e) => Ok(Some(crate::i18n::tp("msg.paste_failed", &[("error", &e.to_string())]))),
    }
}

/// No clipboard of this machine's to read off Windows: a runtime on a server
/// is never pasted into this way (a phone pastes on the phone)
#[cfg(not(windows))]
pub fn clipboard_into(_t: &Tab) -> anyhow::Result<Option<String>> {
    Ok(None)
}

/// The picture on the clipboard sent up to the folder of a tab on another
/// machine, and its path there typed into the tab. `None` when the tab is on
/// this PC or the clipboard holds no picture: pasted as before
#[cfg(windows)]
fn image_far(t: &Tab) -> Option<anyhow::Result<Option<String>>> {
    let machine = match (t.remote(), t.cloud()) {
        (Some(spec), _) => crate::elsewhere::Elsewhere::Ssh(spec.clone()),
        (None, Some(host)) => crate::elsewhere::Elsewhere::Cloud(host.clone()),
        (None, None) => return None,
    };
    let there = t.remote_cwd()?.to_string();
    let image = arboard::Clipboard::new().and_then(|mut c| c.get_image()).ok()?;
    let mut png = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut png, image.width as u32, image.height as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let written = enc.write_header().and_then(|mut w| w.write_image_data(&image.bytes));
        if let Err(e) = written {
            return Some(Ok(Some(crate::i18n::tp("msg.paste_failed", &[("error", &e.to_string())]))));
        }
    }
    use base64::Engine as _;
    let data = base64::engine::general_purpose::STANDARD.encode(&png);
    let said = crate::remote::attach_save_at("", Some((&machine, &there)), "pasted.png", &data);
    Some(match said.get("path").and_then(|p| p.as_str()) {
        Some(path) => t.write_bytes(path.as_bytes()).map(|_| None),
        None => Ok(Some(crate::i18n::tp(
            "msg.paste_failed",
            &[("error", said.get("error").and_then(|e| e.as_str()).unwrap_or_default())],
        ))),
    })
}

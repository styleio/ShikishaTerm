//! A key, as the bytes a program in a tab receives.
//!
//! Terminal work rather than window work: what `Ctrl+Left` is on the wire, what
//! changes when a program has asked for the disambiguating protocol, and which
//! keys a shell swallows. Nothing here knows how the key was pressed -- a
//! window, a phone's soft keyboard and a script all arrive at the same place.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

/// distinction it had no way to make.
/// A modified Enter, Tab, Backspace or Escape, spelled so the program can tell
/// which one it was.
///
/// The old keyboard has no room for these: Enter is one byte, and Shift+Enter
/// is the same byte, so "send this" and "start a new line here" arrive as the
/// same keystroke. Every AI CLI wants both, which is why they ask for the
/// newer keyboard on startup -- and why, until they were answered, the only way
/// to type a newline into one was to know its private workaround.
///
/// Only these four, and only when modified. A program that asked to tell keys
/// apart still gets its ordinary Return as a Return; what it gains is the one
pub fn disambiguated_key(key: &KeyEvent) -> Option<Vec<u8>> {
    let code = match key.code {
        KeyCode::Enter => 13u32,
        KeyCode::Tab | KeyCode::BackTab => 9,
        KeyCode::Backspace => 127,
        KeyCode::Esc => 27,
        _ => return None,
    };
    // One bit per modifier, and the count is that plus one -- which is how
    // every escape has counted modifiers since long before this protocol, and
    // why an unmodified key counts 1 and is not sent this way at all
    let mut bits = 0u8;
    if key.modifiers.contains(KeyModifiers::SHIFT) || key.code == KeyCode::BackTab {
        bits |= 1;
    }
    if key.modifiers.contains(KeyModifiers::ALT) {
        bits |= 2;
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        bits |= 4;
    }
    (bits != 0).then(|| format!("\x1b[{code};{}u", bits + 1).into_bytes())
}

/// crossterm KeyEvent -> the byte sequence sent to the child PTY.
///
/// `flags` is what the program in that tab asked the keyboard to report; 0 is
/// the keyboard every terminal has always had, and what everything not asking
/// for anything gets.
pub fn key_to_bytes_with(key: &KeyEvent, flags: u8) -> Option<Vec<u8>> {
    if flags & 1 != 0 {
        if let Some(bytes) = disambiguated_key(key) {
            return Some(bytes);
        }
    }
    key_to_bytes(key)
}

/// crossterm KeyEvent -> the byte sequence sent to the child PTY (VT100/xterm-style encoding)
pub fn key_to_bytes(key: &KeyEvent) -> Option<Vec<u8>> {
    let mut buf: Vec<u8> = Vec::with_capacity(8);
    if key.modifiers.contains(KeyModifiers::ALT) {
        buf.push(0x1b);
    }
    match key.code {
        KeyCode::Char(c) => {
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                let lower = c.to_ascii_lowercase();
                if lower.is_ascii_lowercase() {
                    buf.push((lower as u8) - b'a' + 1);
                } else {
                    return None;
                }
            } else {
                let mut tmp = [0u8; 4];
                buf.extend_from_slice(c.encode_utf8(&mut tmp).as_bytes());
            }
        }
        KeyCode::Enter => buf.push(b'\r'),
        KeyCode::Backspace => buf.push(0x7f),
        KeyCode::Tab => buf.push(b'\t'),
        KeyCode::BackTab => buf.extend_from_slice(b"\x1b[Z"),
        KeyCode::Esc => buf.push(0x1b),
        KeyCode::Up => buf.extend_from_slice(b"\x1b[A"),
        KeyCode::Down => buf.extend_from_slice(b"\x1b[B"),
        KeyCode::Right => buf.extend_from_slice(b"\x1b[C"),
        KeyCode::Left => buf.extend_from_slice(b"\x1b[D"),
        KeyCode::Home => buf.extend_from_slice(b"\x1b[H"),
        KeyCode::End => buf.extend_from_slice(b"\x1b[F"),
        KeyCode::PageUp => buf.extend_from_slice(b"\x1b[5~"),
        KeyCode::PageDown => buf.extend_from_slice(b"\x1b[6~"),
        KeyCode::Insert => buf.extend_from_slice(b"\x1b[2~"),
        KeyCode::Delete => buf.extend_from_slice(b"\x1b[3~"),
        KeyCode::F(n) => match n {
            1 => buf.extend_from_slice(b"\x1bOP"),
            2 => buf.extend_from_slice(b"\x1bOQ"),
            3 => buf.extend_from_slice(b"\x1bOR"),
            4 => buf.extend_from_slice(b"\x1bOS"),
            5 => buf.extend_from_slice(b"\x1b[15~"),
            6 => buf.extend_from_slice(b"\x1b[17~"),
            7 => buf.extend_from_slice(b"\x1b[18~"),
            8 => buf.extend_from_slice(b"\x1b[19~"),
            9 => buf.extend_from_slice(b"\x1b[20~"),
            10 => buf.extend_from_slice(b"\x1b[21~"),
            11 => buf.extend_from_slice(b"\x1b[23~"),
            12 => buf.extend_from_slice(b"\x1b[24~"),
            _ => return None,
        },
        _ => return None,
    }
    Some(buf)
}

/// Converts a control key sent by name into the terminal's key type
pub fn named_key(n: &str) -> Option<KeyCode> {
    Some(match n {
        "enter" => KeyCode::Enter,
        "bs" => KeyCode::Backspace,
        "tab" => KeyCode::Tab,
        // Both spellings: the page and the phone have always sent "escape"
        // through their own map, and a name that works in one place and is
        // silently ignored in another is the worst kind of half-support
        "esc" | "escape" => KeyCode::Esc,
        "del" => KeyCode::Delete,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "right" => KeyCode::Right,
        "left" => KeyCode::Left,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pgup" => KeyCode::PageUp,
        "pgdn" => KeyCode::PageDown,
        _ => {
            let f = n.strip_prefix('f')?.parse::<u8>().ok()?;
            (1..=12).contains(&f).then_some(KeyCode::F(f))?
        }
    })
}

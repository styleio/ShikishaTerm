//! A key, as the bytes a program in a tab receives.
//!
//! Terminal work rather than window work: what `Ctrl+Left` is on the wire, what
//! changes when a program has asked for the disambiguating protocol, and which
//! keys a shell swallows. Nothing here knows how the key was pressed -- a
//! window, a phone's soft keyboard and a script all arrive at the same place.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

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
    key_to_bytes_for(key, flags, false)
}

/// The same, for a tab whose program may have put the cursor keys in
/// application mode (`CSI ? 1 h`): `application` is whether it has.
///
/// A full-screen program -- vim, less, a file manager -- asks for that mode
/// and then reads the arrows, Home and End as `ESC O A` and not `ESC [ A`.
/// Sent the other spelling, some take the arrow for an Escape followed by
/// letters: the cursor does not move and the letters are typed. Only the
/// bare keys change; one pressed with a modifier is spelled the same in
/// both modes
pub fn key_to_bytes_for(key: &KeyEvent, flags: u8, application: bool) -> Option<Vec<u8>> {
    if flags & 1 != 0
        && let Some(bytes) = disambiguated_key(key) {
            return Some(bytes);
        }
    if application
        && modifier_count(key.modifiers).is_none()
        && let Some(letter) = cursor_letter(key.code)
    {
        return Some(vec![0x1b, b'O', letter]);
    }
    key_to_bytes(key)
}

/// The last letter of an arrow, Home or End as an escape sequence
fn cursor_letter(code: KeyCode) -> Option<u8> {
    Some(match code {
        KeyCode::Up => b'A',
        KeyCode::Down => b'B',
        KeyCode::Right => b'C',
        KeyCode::Left => b'D',
        KeyCode::Home => b'H',
        KeyCode::End => b'F',
        _ => return None,
    })
}

/// The modifiers held, as an escape sequence counts them: one more than a
/// bit each for Shift, Alt and Ctrl. `None` with none held, when the count
/// is left out altogether
fn modifier_count(mods: KeyModifiers) -> Option<u8> {
    let mut bits = 0u8;
    if mods.contains(KeyModifiers::SHIFT) {
        bits |= 1;
    }
    if mods.contains(KeyModifiers::ALT) {
        bits |= 2;
    }
    if mods.contains(KeyModifiers::CONTROL) {
        bits |= 4;
    }
    (bits != 0).then_some(bits + 1)
}

/// A key that moves or edits -- an arrow, Home, End, Page Up and Down,
/// Insert, Delete, a function key -- pressed with Shift, Alt or Ctrl.
///
/// These have had a spelling for decades, the one xterm gave them: the
/// count of modifiers goes in as a parameter (`ESC [ 1 ; 5 D` is Ctrl+Left).
/// Sent as the bare key, Ctrl+Left was Left: no jumping a word at a time in
/// a shell, no Shift+arrow selection in an editor, no Alt+arrow in the AI
/// CLIs that switch on it
fn modified_key(code: KeyCode, count: u8) -> Option<Vec<u8>> {
    let tilde = |n: u8| format!("\x1b[{n};{count}~").into_bytes();
    let letter = |l: u8| format!("\x1b[1;{count}{}", l as char).into_bytes();
    Some(match code {
        KeyCode::PageUp => tilde(5),
        KeyCode::PageDown => tilde(6),
        KeyCode::Insert => tilde(2),
        KeyCode::Delete => tilde(3),
        KeyCode::F(n @ 1..=4) => letter(b'P' + n - 1),
        KeyCode::F(n @ 5..=12) => tilde([15, 17, 18, 19, 20, 21, 23, 24][n as usize - 5]),
        other => letter(cursor_letter(other)?),
    })
}

/// crossterm KeyEvent -> the byte sequence sent to the child PTY (VT100/xterm-style encoding)
pub fn key_to_bytes(key: &KeyEvent) -> Option<Vec<u8>> {
    if let Some(count) = modifier_count(key.modifiers)
        && let Some(bytes) = modified_key(key.code, count)
    {
        return Some(bytes);
    }
    let mut buf: Vec<u8> = Vec::with_capacity(8);
    if key.modifiers.contains(KeyModifiers::ALT) {
        buf.push(0x1b);
    }
    match key.code {
        KeyCode::Char(c) => {
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                let lower = c.to_ascii_lowercase();
                // The control characters, as every terminal types them: a
                // letter is its place in the alphabet, and the handful of
                // signs that follow Z in ASCII are the rest (Ctrl+[ is
                // Escape, Ctrl+Space is NUL)
                buf.push(match lower {
                    'a'..='z' => (lower as u8) - b'a' + 1,
                    ' ' | '@' => 0x00,
                    '[' => 0x1b,
                    '\\' => 0x1c,
                    ']' => 0x1d,
                    '^' => 0x1e,
                    '_' | '/' => 0x1f,
                    '?' => 0x7f,
                    _ => return None,
                });
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A Ctrl chord is a control character: a letter by its place in the
    /// alphabet, and the signs after Z by theirs. One that is none is nothing
    #[test]
    fn a_ctrl_chord_is_the_control_character_every_terminal_types() {
        let ctrl = |c: char| key_to_bytes(&KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL));
        assert_eq!(ctrl('c'), Some(vec![0x03]));
        assert_eq!(ctrl('C'), Some(vec![0x03]));
        assert_eq!(ctrl(' '), Some(vec![0x00]), "Ctrl+Space");
        assert_eq!(ctrl('['), Some(vec![0x1b]), "Ctrl+[ is Escape");
        assert_eq!(ctrl('\\'), Some(vec![0x1c]));
        assert_eq!(ctrl(']'), Some(vec![0x1d]));
        assert_eq!(ctrl('_'), Some(vec![0x1f]));
        assert_eq!(ctrl('с'), None, "a letter of another script is no control character");
    }

    /// A moving or editing key pressed with a modifier says so, the way
    /// every terminal has spelled it; bare, it is what it always was
    #[test]
    fn a_modified_arrow_says_which_modifiers_were_held() {
        let k = |code, mods| key_to_bytes(&KeyEvent::new(code, mods)).map(|b| String::from_utf8(b).unwrap());
        let (s, a, c) = (KeyModifiers::SHIFT, KeyModifiers::ALT, KeyModifiers::CONTROL);
        assert_eq!(k(KeyCode::Left, KeyModifiers::NONE).as_deref(), Some("\x1b[D"));
        assert_eq!(k(KeyCode::Left, c).as_deref(), Some("\x1b[1;5D"), "Ctrl+Left is a plain Left");
        assert_eq!(k(KeyCode::Right, s).as_deref(), Some("\x1b[1;2C"));
        assert_eq!(k(KeyCode::Up, a).as_deref(), Some("\x1b[1;3A"));
        assert_eq!(k(KeyCode::Down, c | s).as_deref(), Some("\x1b[1;6B"));
        assert_eq!(k(KeyCode::Home, c).as_deref(), Some("\x1b[1;5H"));
        assert_eq!(k(KeyCode::End, s | a | c).as_deref(), Some("\x1b[1;8F"));
        assert_eq!(k(KeyCode::Delete, c).as_deref(), Some("\x1b[3;5~"));
        assert_eq!(k(KeyCode::PageUp, s).as_deref(), Some("\x1b[5;2~"));
        assert_eq!(k(KeyCode::F(1), s).as_deref(), Some("\x1b[1;2P"));
        assert_eq!(k(KeyCode::F(4), c).as_deref(), Some("\x1b[1;5S"));
        assert_eq!(k(KeyCode::F(5), c).as_deref(), Some("\x1b[15;5~"));
        assert_eq!(k(KeyCode::F(12), a).as_deref(), Some("\x1b[24;3~"));
        // The keys with no such spelling keep theirs
        assert_eq!(k(KeyCode::Char('b'), a).as_deref(), Some("\x1bb"));
        assert_eq!(k(KeyCode::Enter, a).as_deref(), Some("\x1b\r"));
        assert_eq!(k(KeyCode::BackTab, s).as_deref(), Some("\x1b[Z"));
        assert_eq!(k(KeyCode::Tab, c).as_deref(), Some("\t"));
    }

    /// A program that put the cursor keys in application mode reads the
    /// bare ones in that mode's spelling; a modified one is the same in both
    #[test]
    fn a_program_in_application_mode_is_sent_its_own_arrows() {
        let k = |code, mods, app| key_to_bytes_for(&KeyEvent::new(code, mods), 0, app).map(|b| String::from_utf8(b).unwrap());
        let none = KeyModifiers::NONE;
        for (code, letter) in [(KeyCode::Up, 'A'), (KeyCode::Down, 'B'), (KeyCode::Right, 'C'), (KeyCode::Left, 'D'), (KeyCode::Home, 'H'), (KeyCode::End, 'F')] {
            assert_eq!(k(code, none, false), Some(format!("\x1b[{letter}")));
            assert_eq!(k(code, none, true), Some(format!("\x1bO{letter}")), "{code:?} ignores the mode the program asked for");
        }
        assert_eq!(k(KeyCode::Left, KeyModifiers::CONTROL, true).as_deref(), Some("\x1b[1;5D"));
        assert_eq!(k(KeyCode::PageUp, none, true).as_deref(), Some("\x1b[5~"));
        assert_eq!(k(KeyCode::Char('a'), none, true).as_deref(), Some("a"));
        // The newer keyboard's four keys come first, as without the mode
        assert_eq!(key_to_bytes_for(&KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT), 1, true), Some(b"\x1b[13;2u".to_vec()));
    }
}

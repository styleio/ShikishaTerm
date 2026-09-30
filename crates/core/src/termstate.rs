//! A terminal's state, handed from the process that holds the terminal to a
//! window that comes back to it (far-keep plan §4.4).
//!
//! The holder keeps a parser of its own beside the terminal. A window that
//! comes back takes the holder's state -- both screens, cursors, scroll
//! region, modes, as `vt100` writes it ([`vt100::Screen::snapshot`]) -- in
//! place of its own, and from then on reads the terminal's raw output like
//! any other. For the two to stay the same after that, the state has to be
//! taken where the output is between two whole things: a character, or an
//! escape sequence. The parser inside `vt100` does not say where it is in a
//! sequence, so the holder looks at the bytes itself before they reach it
//! ([`Boundary`]): what is whole goes to its parser, a started character or
//! sequence at the end waits, and goes with the state -- the window feeds it
//! to its own parser after taking the state, and the next output carries on
//! from it there.

/// The longest unfinished tail kept back. An OSC that never ends is let
/// through to the parser at this length, which cuts it short itself, so the
/// terminal is never held up waiting for an end that does not come. Larger
/// than any OSC the parser keeps whole
pub const PENDING_MOST: usize = 64 * 1024;

/// Where the output stands: between whole things, or partway into one
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum At {
    /// Between whole things
    #[default]
    Ground,
    /// Inside a UTF-8 character, this many bytes still to come
    Utf8(u8),
    /// After ESC
    Esc,
    /// After ESC and intermediate bytes (0x20..=0x2f)
    EscIntermediate,
    /// A control sequence (ESC [)
    Csi,
    /// A string ended by ST or BEL: OSC (ESC ]), and DCS, SOS, PM, APC,
    /// which end at ST only
    Str { bel_ends: bool },
    /// ESC seen inside a string: ST if a backslash follows
    StrEsc { bel_ends: bool },
}

/// Splits a terminal's output at the end of the last whole thing in it
#[derive(Debug, Default, Clone)]
pub struct Boundary {
    at: At,
    /// What of the output has been seen and not yet let through
    pending: Vec<u8>,
}

impl Boundary {
    /// The part of `bytes` (after what was held back before) that ends on a
    /// whole thing; the rest is held back. What is handed out goes to the
    /// holder's parser; what is held back goes with the state
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<u8> {
        self.pending.extend_from_slice(bytes);
        let mut whole = 0;
        let start = self.pending.len() - bytes.len();
        for i in start..self.pending.len() {
            self.at = step(self.at, self.pending[i]);
            if self.at == At::Ground {
                whole = i + 1;
            }
        }
        // Everything before `start` was already found to be unfinished; a
        // byte that finishes it makes all of it whole
        let out: Vec<u8> = if whole > 0 { self.pending.drain(..whole).collect() } else { Vec::new() };
        if self.pending.len() > PENDING_MOST {
            // Never held up: the parser cuts a sequence this long short itself
            self.at = At::Ground;
            let mut all = out;
            all.append(&mut self.pending);
            return all;
        }
        out
    }

    /// What is held back now: to go with the state, and to be fed to the
    /// receiver's parser after it took the state
    pub fn pending(&self) -> &[u8] {
        &self.pending
    }
}

/// One byte further. Written from ECMA-48 and the xterm control sequences,
/// the same grammar `vte` follows, for where a thing ends and nothing more
fn step(at: At, b: u8) -> At {
    // CAN and SUB end any sequence; ESC starts a new one
    match (at, b) {
        (At::Str { bel_ends }, 0x1b) => return At::StrEsc { bel_ends },
        (At::StrEsc { .. }, b'\\') => return At::Ground,
        (At::StrEsc { bel_ends }, _) => return At::Str { bel_ends },
        (At::Str { bel_ends: true }, 0x07) => return At::Ground,
        (At::Str { .. }, _) => return at,
        (_, 0x18 | 0x1a) => return At::Ground,
        (_, 0x1b) => return At::Esc,
        _ => {}
    }
    match at {
        At::Ground => match b {
            0xc0..=0xdf => At::Utf8(1),
            0xe0..=0xef => At::Utf8(2),
            0xf0..=0xf7 => At::Utf8(3),
            _ => At::Ground,
        },
        At::Utf8(left) => match b {
            0x80..=0xbf if left > 1 => At::Utf8(left - 1),
            0x80..=0xbf => At::Ground,
            // Not a continuation: the character was cut short, and this
            // byte starts afresh
            _ => step(At::Ground, b),
        },
        At::Esc => match b {
            b'[' => At::Csi,
            b']' => At::Str { bel_ends: true },
            b'P' | b'X' | b'^' | b'_' => At::Str { bel_ends: false },
            0x20..=0x2f => At::EscIntermediate,
            // A C0 control inside a sequence is carried out and the sequence goes on
            0x00..=0x1f => At::Esc,
            _ => At::Ground,
        },
        At::EscIntermediate => match b {
            0x20..=0x2f | 0x00..=0x1f => At::EscIntermediate,
            _ => At::Ground,
        },
        At::Csi => match b {
            0x40..=0x7e => At::Ground,
            _ => At::Csi,
        },
        At::Str { .. } | At::StrEsc { .. } => unreachable!("handled above"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Output that goes through everything a terminal's state holds: colours
    /// and attributes, wide and combining characters, a hyperlink, a scroll
    /// region with lines scrolled inside it, origin mode, saved cursors both
    /// ways, the alternate screen in and out, bracketed paste, application
    /// cursor keys and keypad, mouse reporting, a line filled to the last
    /// column, and enough lines to have scrollback
    fn stream() -> Vec<&'static [u8]> {
        let mut s: Vec<&'static [u8]> = vec![
            b"plain line\r\n",
            b"\x1b[31mred\x1b[0m \x1b[38;5;100mindexed\x1b[m \x1b[38;2;1;2;3mrgb\x1b[1;3;4;7m all\x1b[m\r\n",
            "日本語 é wide\r\n".as_bytes(),
            b"\x1b]8;;https://example.com\x1b\\linked\x1b]8;;\x1b\\ after\r\n",
            b"\x1b]0;a title\x07",
            b"\x1b[?2004h\x1b[?1h\x1b=\x1b[?1002h\x1b[?1006h",
            b"\x1b7\x1b[5;10Hsaved-at\x1b8back\r\n",
            b"\x1b[2;6r\x1b[?6h\x1b[1;1Hinside\r\n",
            b"one\r\ntwo\r\nthree\r\nfour\r\nfive\r\nsix\r\n",
            b"\x1b[?6l\x1b[r",
            b"\x1b[?1049h\x1b[H\x1b[2Jalternate\r\n\x1b[s\x1b[10;20Hthere\x1b[u",
            b"\x1b[?1049l",
            b"0123456789012345678901234567890123456789012345678901234567890123456789012345678",
            b"9",
            b"next\r\n",
        ];
        for _ in 0..40 {
            s.push(b"scrolling line\r\n");
        }
        s.push(b"\x1b[?2004l\x1b[?1l\x1b>\x1b[?1002l\x1b[?1006l end");
        s
    }

    /// The state taken at any point between whole things and put into a new
    /// parser, both then fed the rest of the output: they stay the same --
    /// both screens, cursors, modes -- to the end, and at every step between
    #[test]
    fn a_state_moved_to_another_parser_stays_the_same_as_the_output_goes_on() {
        let parts = stream();
        for cut in 0..=parts.len() {
            let mut kept = vt100::Parser::new(24, 80, 100);
            for p in &parts[..cut] {
                kept.process(p);
            }
            let mut moved = vt100::Parser::new(24, 80, 100);
            moved.restore(vt100::Screen::from_snapshot(&kept.screen().snapshot(100)).unwrap());
            assert_eq!(kept.screen().snapshot(100), moved.screen().snapshot(100), "not the same when moved, at {cut}");
            for (i, p) in parts[cut..].iter().enumerate() {
                kept.process(p);
                moved.process(p);
                assert_eq!(
                    kept.screen().snapshot(100),
                    moved.screen().snapshot(100),
                    "apart after moving at {cut}, {i} parts later"
                );
            }
            assert_eq!(kept.screen().contents_formatted(), moved.screen().contents_formatted());
            assert_eq!(kept.screen().cursor_position(), moved.screen().cursor_position());
        }
    }

    /// The state taken at ANY byte -- inside a character, inside a sequence --
    /// with the held-back tail going along: the receiver takes the state,
    /// feeds itself the tail, and from then on the two stay the same
    #[test]
    fn a_state_taken_mid_sequence_carries_its_tail_and_stays_the_same() {
        let all: Vec<u8> = stream().concat();
        for cut in 0..=all.len() {
            let mut holder = vt100::Parser::new(24, 80, 100);
            let mut edge = Boundary::default();
            holder.process(&edge.feed(&all[..cut]));
            // Handed over: the state and the tail
            let mut window = vt100::Parser::new(24, 80, 100);
            window.restore(vt100::Screen::from_snapshot(&holder.screen().snapshot(100)).unwrap());
            window.process(edge.pending());
            // What the reference parser, fed everything in one go, holds
            let mut whole = vt100::Parser::new(24, 80, 100);
            whole.process(&all);
            window.process(&all[cut..]);
            assert_eq!(
                whole.screen().snapshot(100),
                window.screen().snapshot(100),
                "a window that came back at byte {cut} is not the same"
            );
        }
    }

    /// Where the output ends on a whole thing, nothing is held back; where it
    /// does not, exactly the unfinished part is
    #[test]
    fn only_the_unfinished_tail_is_held_back() {
        let cases: &[(&[u8], &[u8])] = &[
            (b"abc", b""),
            (b"abc\x1b", b"\x1b"),
            (b"abc\x1b[3", b"\x1b[3"),
            (b"abc\x1b[31m", b""),
            (b"x\x1b]0;title", b"\x1b]0;title"),
            (b"x\x1b]0;title\x07", b""),
            (b"x\x1b]8;;u\x1b", b"\x1b]8;;u\x1b"),
            (b"x\x1b]8;;u\x1b\\", b""),
            ("日".as_bytes().split_at(2).0, "日".as_bytes().split_at(2).0),
            ("日".as_bytes(), b""),
            (b"\x1bP1$r", b"\x1bP1$r"),
            (b"\x1b(B", b""),
        ];
        for (fed, held) in cases {
            let mut b = Boundary::default();
            let out = b.feed(fed);
            assert_eq!(b.pending(), *held, "{:?}", String::from_utf8_lossy(fed));
            assert_eq!([out.as_slice(), b.pending()].concat(), fed.to_vec());
        }
    }

    /// A string that never ends is not held for ever: past the limit it goes
    /// through, and what follows is read as it should be
    #[test]
    fn an_unending_string_is_let_through_at_the_limit() {
        let mut b = Boundary::default();
        let mut long = b"\x1b]0;".to_vec();
        long.extend(std::iter::repeat_n(b'x', PENDING_MOST + 10));
        let out = b.feed(&long);
        assert_eq!(out.len(), long.len(), "held up by a string with no end");
        assert!(b.pending().is_empty());
        assert_eq!(b.feed(b"\x1b[1mok"), b"\x1b[1mok".to_vec());
    }
}

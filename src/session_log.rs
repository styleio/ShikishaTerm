//! Session log: keeps what scrolled across the screen in a text file.
//! Equivalent to PuTTY's "session logging"; lets you trace the exchange
//! with an AI afterward.

use std::io::Write as _;
use std::time::{SystemTime, UNIX_EPOCH};

/// Strips terminal control sequences, leaving only human-readable text.
pub struct AnsiStripper {
    state: State,
}

enum State {
    Text,
    Esc,
    /// CSI (ESC [) — skip until the terminator byte (0x40..=0x7e)
    Csi,
    /// OSC (ESC ]) — skip until BEL or ESC \
    Osc,
    OscEsc,
}

impl Default for AnsiStripper {
    fn default() -> Self {
        Self { state: State::Text }
    }
}

impl AnsiStripper {
    pub fn feed(&mut self, bytes: &[u8], out: &mut Vec<u8>) {
        for &b in bytes {
            match self.state {
                State::Text => match b {
                    0x1b => self.state = State::Esc,
                    b'\n' | b'\t' => out.push(b),
                    // CR is a carriage return; drop it so it doesn't leave overwrite-redraw artifacts
                    b'\r' => {}
                    0x00..=0x1f | 0x7f => {}
                    _ => out.push(b),
                },
                State::Esc => match b {
                    b'[' => self.state = State::Csi,
                    b']' => self.state = State::Osc,
                    // An ESC + 1-character sequence
                    _ => self.state = State::Text,
                },
                State::Csi => {
                    if (0x40..=0x7e).contains(&b) {
                        self.state = State::Text;
                    }
                }
                State::Osc => match b {
                    0x07 => self.state = State::Text,
                    0x1b => self.state = State::OscEsc,
                    _ => {}
                },
                State::OscEsc => self.state = State::Text,
            }
        }
    }
}

/// An append-only session log. Silently disables itself if it can't write
/// (never let logging bring down a session).
pub struct SessionLog {
    file: Option<std::fs::File>,
    stripper: AnsiStripper,
    buf: Vec<u8>,
}

impl SessionLog {
    pub fn open(dir: &std::path::Path, title: &str) -> Self {
        let _ = std::fs::create_dir_all(dir);
        let name = format!("{}-{}.log", sanitize(title), today());
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join(name))
            .ok();
        let mut me = Self {
            file,
            stripper: AnsiStripper::default(),
            buf: Vec::new(),
        };
        me.raw(&crate::i18n::tp(
            "log.session.start",
            &[("time", &now_string())],
        ));
        me
    }

    pub fn write(&mut self, bytes: &[u8]) {
        let Some(f) = self.file.as_mut() else { return };
        self.buf.clear();
        self.stripper.feed(bytes, &mut self.buf);
        if !self.buf.is_empty() {
            let _ = f.write_all(&self.buf);
        }
    }

    fn raw(&mut self, s: &str) {
        if let Some(f) = self.file.as_mut() {
            let _ = f.write_all(s.as_bytes());
        }
    }
}

fn sanitize(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.trim_matches('_').is_empty() {
        "session".into()
    } else {
        cleaned
    }
}

fn epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Compute the calendar date from epoch seconds (the civil_from_days
/// algorithm).
fn ymd(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn today() -> String {
    let (y, m, d) = ymd((epoch_secs() / 86_400) as i64);
    format!("{y:04}{m:02}{d:02}")
}

fn now_string() -> String {
    let secs = epoch_secs();
    let (y, m, d) = ymd((secs / 86_400) as i64);
    let t = secs % 86_400;
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02} UTC",
        t / 3600,
        (t % 3600) / 60,
        t % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strip(input: &[u8]) -> String {
        let mut s = AnsiStripper::default();
        let mut out = Vec::new();
        s.feed(input, &mut out);
        String::from_utf8_lossy(&out).into_owned()
    }

    #[test]
    fn removes_control_sequences_but_keeps_text() {
        assert_eq!(strip(b"\x1b[31mhello\x1b[0m\n"), "hello\n");
        assert_eq!(strip(b"\x1b]0;title\x07body"), "body");
        assert_eq!(strip(b"a\rb\n"), "ab\n");
        assert_eq!(strip("日本語\x1b[K\n".as_bytes()), "日本語\n");
    }

    #[test]
    fn handles_sequences_split_across_chunks() {
        let mut s = AnsiStripper::default();
        let mut out = Vec::new();
        s.feed(b"\x1b[3", &mut out);
        s.feed(b"1mred", &mut out);
        assert_eq!(String::from_utf8_lossy(&out), "red");
    }

    #[test]
    fn date_conversion_is_correct() {
        assert_eq!(ymd(0), (1970, 1, 1));
        assert_eq!(ymd(19_000), (2022, 1, 8));
        // Japanese text is kept in the file name (so it's clear which tab a
        // log belongs to). Only characters not usable in a file name are
        // replaced.
        assert_eq!(sanitize("A:実装"), "A_実装");
        assert_eq!(sanitize("///"), "session");
    }
}

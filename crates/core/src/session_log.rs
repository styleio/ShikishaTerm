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
    /// Where it is being written. Kept so automation can read the run back from
    /// a mark of its own -- see `read_from`
    path: Option<std::path::PathBuf>,
    stripper: AnsiStripper,
    buf: Vec<u8>,
}

impl SessionLog {
    pub fn open(dir: &std::path::Path, title: &str) -> Self {
        let _ = std::fs::create_dir_all(dir);
        let name = format!("{}-{}.log", sanitize(title), today());
        let path = dir.join(name);
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok();
        let mut me = Self {
            path: file.is_some().then_some(path),
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


/// Where this log is being written, if it is.
impl SessionLog {
    pub fn path(&self) -> Option<&std::path::Path> {
        self.path.as_deref()
    }
}

/// Read a recording from a mark, and hand back the next one.
///
/// The mark is a position in the file, which is what makes "from where I left
/// off" cheap: no scanning, no remembering what was already seen. A run that
/// printed four thousand lines is read in whatever pieces the caller asks for.
///
/// A read is bounded -- a caller that has been away a long time gets the next
/// piece, not all of it -- and the mark it gets back is where that piece ended,
/// so the next call continues from there.
///
/// A partial character at the end of a piece is a real possibility (the mark is
/// in bytes, and the text is not) so the read backs up to the last whole one
/// rather than handing over a broken character.
pub fn read_from(path: &std::path::Path, from: u64) -> (String, u64) {
    use std::io::{Read as _, Seek as _};
    /// Most a single read hands back
    const MAX: usize = 64 * 1024;
    let Ok(mut f) = std::fs::File::open(path) else {
        return (String::new(), from);
    };
    // A recording that was rotated or cleared is shorter than the mark: start
    // over rather than reading nothing for ever
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let from = if from > len { 0 } else { from };
    if f.seek(std::io::SeekFrom::Start(from)).is_err() {
        return (String::new(), from);
    }
    let mut buf = vec![0u8; MAX];
    let read = match f.read(&mut buf) {
        Ok(n) => n,
        Err(_) => return (String::new(), from),
    };
    buf.truncate(read);
    // Back up to the last whole character, unless that would mean handing back
    // nothing at all (a single character longer than the buffer cannot happen,
    // but a caller stuck at the same mark for ever would be worse than a
    // replacement glyph)
    let good = match std::str::from_utf8(&buf) {
        Ok(_) => read,
        Err(e) if e.valid_up_to() > 0 => e.valid_up_to(),
        Err(_) => read,
    };
    let text = String::from_utf8_lossy(&buf[..good]).to_string();
    (text, from + good as u64)
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

    /// Reading a long run back in pieces, from a mark the caller keeps.
    #[test]
    fn a_recording_is_read_from_a_mark_and_hands_the_next_one_back() {
        let dir = std::env::temp_dir().join("shikisha-log-read");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("run.log");
        std::fs::write(&path, "one
two
").unwrap();

        let (text, at) = super::read_from(&path, 0);
        assert_eq!(text, "one
two
");
        assert_eq!(at, 8, "次の印が末尾になっていない");

        // Nothing new yet: the mark comes straight back
        let (text, again) = super::read_from(&path, at);
        assert_eq!(text, "");
        assert_eq!(again, at, "読むものが無いのに印が動いている");

        // ...and only what was added since
        std::fs::write(&path, "one
two
three
").unwrap();
        let (text, _) = super::read_from(&path, at);
        assert_eq!(text, "three
", "同じところを二度読んでいる");

        // A recording that was cleared is shorter than the mark. Start over
        // rather than reading nothing for ever
        std::fs::write(&path, "fresh
").unwrap();
        let (text, at) = super::read_from(&path, 999);
        assert_eq!(text, "fresh
", "短くなった記録から読み直していない");
        assert_eq!(at, 6);

        // A character split across the end of a piece is not handed over broken
        std::fs::write(&path, "あい").unwrap();
        let (text, at) = super::read_from(&path, 0);
        assert_eq!(text, "あい");
        assert_eq!(at, 6);

        // A file that is not there reads as nothing, and keeps the mark
        let (text, at) = super::read_from(&dir.join("nope.log"), 42);
        assert_eq!((text.as_str(), at), ("", 42));
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

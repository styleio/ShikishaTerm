//! A tab = one PTY session (child process + vt100 parser + state detection). DESIGN.md chapter 4.

use std::io::{Read as _, Write as _};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::Result;
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};

use crate::detect::{Detector, TabState};
use crate::profile::Profile;

pub const SCROLLBACK_LINES: usize = 5000;

/// Per-tab terminal settings (roughly equivalent to a PuTTY session profile)
#[derive(Clone)]
pub struct TabOptions {
    /// Working folder at launch (the AI CLI looks at the project here)
    pub cwd: Option<std::path::PathBuf>,
    /// Number of scrollback lines
    pub scrollback: usize,
    /// Encoding ("utf-8" / "shift_jis" / "euc-jp" etc). Defaults to UTF-8
    pub encoding: Option<&'static encoding_rs::Encoding>,
    /// Save the session log under logs/
    pub log: bool,
    /// If this tab is a model bridge (OpenAI-compatible API), its endpoint.
    /// When Some, spawn does not start a real process — it only starts an idle
    /// placeholder process to hold the display
    pub model: Option<crate::bridge::ModelConn>,
}

impl Default for TabOptions {
    fn default() -> Self {
        Self {
            cwd: None,
            scrollback: SCROLLBACK_LINES,
            encoding: None,
            log: false,
            model: None,
        }
    }
}

impl TabOptions {
    /// Resolve an encoding from a config string (unknown names are treated as UTF-8)
    pub fn encoding_from_name(name: Option<&str>) -> Option<&'static encoding_rs::Encoding> {
        let n = name?.trim();
        if n.is_empty() || n.eq_ignore_ascii_case("utf-8") || n.eq_ignore_ascii_case("utf8") {
            return None;
        }
        encoding_rs::Encoding::for_label(n.as_bytes())
            .filter(|e| *e != encoding_rs::UTF_8)
    }
}

pub type PtyWriter = Arc<Mutex<Box<dyn std::io::Write + Send>>>;
pub type SharedParser = Arc<Mutex<vt100::Parser<QueryResponder>>>;

/// Copy-mode state (started with Ctrl+B [ / mouse)
pub struct CopyState {
    /// Cursor row within the pane (0 = top row)
    pub cursor_row: u16,
    /// Selection start position (row count from the bottom of the screen). None = no selection
    pub anchor: Option<usize>,
    /// What is being typed into the search line, while it is open
    pub find: Option<String>,
    /// What was searched for last, so the search can be repeated after the
    /// line has closed
    pub last: String,
}

/// Responder for terminal queries (DSR/DA) from the child process.
/// Programs under ConPTY (ssh etc) block waiting for a reply to the cursor
/// position query `\x1b[6n`, so we write the reply back to the PTY just like
/// a real terminal would.
/// Also counts bell characters (often used for completion notifications) as
/// a signal for state detection.
pub struct QueryResponder {
    writer: PtyWriter,
    bell: Arc<AtomicU64>,
    /// Notifications the program in this tab asked for, in the standard
    /// escape sequences every terminal understands. Drained by the main loop
    notes: Notes,
}

/// (title, body) pairs waiting to be shown
pub type Notes = Arc<Mutex<Vec<(String, String)>>>;

/// Read a notification out of an OSC sequence, in any of the three spellings
/// terminals have settled on.
///
/// Worth taking all three: this is how a CLI that has never heard of this app
/// still gets to say "I need you" — no profile to write, no hook to install.
/// The parts are simply named differently by each.
fn note_of(params: &[&[u8]]) -> Option<(String, String)> {
    let text = |b: &[u8]| String::from_utf8_lossy(b).trim().to_string();
    let joined = |from: usize| {
        params[from..]
            .iter()
            .map(|p| text(p))
            .collect::<Vec<_>>()
            .join(";")
    };
    match params.first().map(|p| text(p)).as_deref() {
        // \e]9;body\a — the oldest and simplest: a body and nothing else
        Some("9") if params.len() > 1 => Some((String::new(), joined(1))),
        // \e]777;notify;title;body\a
        Some("777") if params.len() > 2 && text(params[1]) == "notify" => {
            Some((text(params[2]), joined(3)))
        }
        // \e]99;<key=value:…>;payload\e\ — metadata, then the text. `p=title`
        // says the payload is the title rather than the body
        Some("99") if params.len() > 1 => {
            let meta = joined(1);
            let (keys, payload) = match meta.rsplit_once(':') {
                Some((k, v)) if k.contains('=') => (k.to_string(), v.to_string()),
                _ => (String::new(), meta),
            };
            if payload.trim().is_empty() {
                return None;
            }
            match keys.contains("p=title") {
                true => Some((payload, String::new())),
                false => Some((String::new(), payload)),
            }
        }
        _ => None,
    }
}

impl QueryResponder {
    fn reply(&self, bytes: &[u8]) {
        if let Ok(mut w) = self.writer.lock() {
            let _ = w.write_all(bytes);
            let _ = w.flush();
        }
    }
}

impl vt100::Callbacks for QueryResponder {
    fn audible_bell(&mut self, _: &mut vt100::Screen) {
        self.bell.fetch_add(1, Ordering::Relaxed);
    }

    /// A program asking to be noticed. Kept rather than dropped, and kept
    /// bounded: a program in a loop must not be able to fill memory with its
    /// own announcements
    fn unhandled_osc(&mut self, _: &mut vt100::Screen, params: &[&[u8]]) {
        let Some(note) = note_of(params) else { return };
        if let Ok(mut n) = self.notes.lock() {
            if n.len() < 32 {
                n.push(note);
            }
        }
    }

    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        i1: Option<u8>,
        _i2: Option<u8>,
        params: &[&[u16]],
        c: char,
    ) {
        let p0 = params.first().and_then(|p| p.first()).copied();
        match (i1, c, p0) {
            // DSR-CPR: cursor position query → \x1b[{row};{col}R (1-based)
            (None, 'n', Some(6)) => {
                let (row, col) = screen.cursor_position();
                self.reply(format!("\x1b[{};{}R", row + 1, col + 1).as_bytes());
            }
            // DSR: terminal status query → OK
            (None, 'n', Some(5)) => self.reply(b"\x1b[0n"),
            // DA1: terminal type query → VT102-equivalent
            (None, 'c', _) => self.reply(b"\x1b[?6c"),
            // DA2: secondary terminal type query
            (Some(b'>'), 'c', _) => self.reply(b"\x1b[>0;0;0c"),
            _ => {}
        }
    }
}

pub fn pty_write(writer: &PtyWriter, bytes: &[u8]) -> Result<()> {
    let mut w = writer.lock().expect("pty writer lock");
    w.write_all(bytes)?;
    Ok(())
}

/// The idle process started solely so a model tab has a display to hold onto.
/// It just sits alive quietly waiting for keystrokes (the screen content is
/// injected by the main process)
fn idle_argv() -> Vec<String> {
    vec!["cmd.exe".into(), "/c".into(), "pause>nul".into()]
}

/// Truncate to at most `max` characters, appending "…" when it had to cut.
/// Character counts (not display width) — fine here since the values are ASCII.
fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut r: String = s.chars().take(max - 1).collect();
    r.push('…');
    r
}

/// A compact, ASCII-art-free title card for a model-bridge tab, in the spirit
/// of the CLIs' startup boxes. It identifies the endpoint (model / provider /
/// host) so a blank idle screen doesn't look broken. The width tracks the
/// terminal so it stays intact on narrow windows instead of wrapping.
fn model_title_box(conn: &crate::bridge::ModelConn, cols: u16) -> String {
    // Host portion of the endpoint URL ("https://api.deepseek.com/v1" -> "api.deepseek.com").
    let host = conn
        .url
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(&conn.url)
        .split('/')
        .next()
        .unwrap_or(&conn.url);
    const HEADER: &str = ">_ SHIKISHA bridge";
    const LABEL_W: usize = 10; // widest label ("provider:"/"endpoint:") + a space
    // (label, value); an empty label is a blank spacer row.
    let rows: [(&str, &str); 5] = [
        ("", ""),
        ("model", &conn.model),
        ("provider", &conn.provider),
        ("endpoint", host),
        ("mode", "internal \u{b7} no ConPTY child"),
    ];
    // Inner text width: fit the longest line, but never exceed the terminal.
    let longest = rows
        .iter()
        .map(|(l, v)| if l.is_empty() { 0 } else { LABEL_W + v.chars().count() })
        .chain(std::iter::once(HEADER.chars().count()))
        .max()
        .unwrap_or(0);
    let max_inner = (cols as usize).saturating_sub(4).max(8);
    let inner = longest.clamp(8, max_inner);
    let bar = "\u{2500}".repeat(inner + 2);
    let line = |body: &str| {
        format!(
            "\x1b[36m\u{2502}\x1b[0m {:<width$} \x1b[36m\u{2502}\x1b[0m\r\n",
            clip(body, inner),
            width = inner
        )
    };
    // Lead with a few blank lines so the header clears the discussion banner
    // (which floats over the top of every tab while a discussion is at rest).
    let mut out = String::from("\r\n\r\n\r\n");
    out.push_str(&format!("\x1b[36m\u{256d}{bar}\u{256e}\x1b[0m\r\n"));
    out.push_str(&format!(
        "\x1b[36m\u{2502}\x1b[0m \x1b[1;36m{:<width$}\x1b[0m \x1b[36m\u{2502}\x1b[0m\r\n",
        clip(HEADER, inner),
        width = inner
    ));
    for (label, value) in rows.iter() {
        if label.is_empty() {
            out.push_str(&line(""));
        } else {
            let value = clip(value, inner.saturating_sub(LABEL_W));
            out.push_str(&line(&format!("{:<LABEL_W$}{value}", format!("{label}:"))));
        }
    }
    out.push_str(&format!("\x1b[36m\u{2570}{bar}\u{256f}\x1b[0m\r\n"));
    out
}

/// Build the launch command.
/// CreateProcess cannot launch extension-less scripts directly (e.g. npm
/// shims) (os error 193), so we search PATH+PATHEXT and route .cmd/.bat
/// through cmd.exe /c
pub fn build_command(cmd_args: &[String]) -> CommandBuilder {
    let Some(prog) = cmd_args.first() else {
        return CommandBuilder::new("powershell.exe");
    };
    let rest = &cmd_args[1..];
    match resolve_windows_command(prog) {
        Some(path) => {
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_ascii_lowercase());
            if matches!(ext.as_deref(), Some("cmd") | Some("bat")) {
                let mut c = CommandBuilder::new("cmd.exe");
                c.arg("/c");
                c.arg(path);
                for a in rest {
                    c.arg(a);
                }
                c
            } else {
                let mut c = CommandBuilder::new(path);
                for a in rest {
                    c.arg(a);
                }
                c
            }
        }
        // If it can't be resolved, pass it through as-is so the error surfaces
        None => {
            let mut c = CommandBuilder::new(prog);
            for a in rest {
                c.arg(a);
            }
            c
        }
    }
}

/// Resolve a command to an actual file via PATH and executable extensions (.exe/.com/.cmd/.bat)
pub fn resolve_command(prog: &str) -> Option<std::path::PathBuf> {
    resolve_windows_command(prog)
}

fn resolve_windows_command(prog: &str) -> Option<std::path::PathBuf> {
    use std::path::{Path, PathBuf};
    const EXTS: [&str; 4] = ["exe", "com", "cmd", "bat"];

    let has_exec_ext = |p: &Path| {
        p.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| EXTS.contains(&e.to_ascii_lowercase().as_str()))
    };
    let try_base = |base: PathBuf| -> Option<PathBuf> {
        if has_exec_ext(&base) && base.is_file() {
            return Some(base);
        }
        EXTS.iter()
            .map(|e| base.with_extension(e))
            .find(|cand| cand.is_file())
    };

    let p = Path::new(prog);
    // A path that already contains a separator is resolved as-is, without searching PATH
    if p.components().count() > 1 {
        return try_base(p.to_path_buf());
    }
    let path_var = std::env::var_os("PATH")?;
    std::env::split_paths(&path_var).find_map(|dir| try_base(dir.join(prog)))
}

/// Turns a raw process-spawn failure into a gentle, plain-language explanation.
///
/// A portable build is often carried to a PC that doesn't have the configured
/// tool installed, or where a saved absolute folder no longer exists. Both come
/// back from the OS as a bare "file not found", so instead of surfacing that,
/// name the likely cause and point at the setting to change.
pub fn launch_problem(
    name: &str,
    prog: &str,
    cwd: Option<&std::path::Path>,
    raw: &str,
) -> String {
    // A missing working folder and a missing program both surface as
    // "file not found", so check the folder directly rather than guess from the
    // OS error code.
    if let Some(dir) = cwd {
        if !dir.as_os_str().is_empty() && !dir.exists() {
            return crate::i18n::tp(
                "msg.start.no_folder",
                &[("name", name), ("path", &dir.display().to_string())],
            );
        }
    }
    if !prog.is_empty() && resolve_command(prog).is_none() {
        return crate::i18n::tp("msg.start.no_command", &[("name", name), ("cmd", prog)]);
    }
    crate::i18n::tp(
        "msg.start.other",
        &[("name", name), ("error", &raw.replace('\0', ""))],
    )
}

/// Return the range to extract as the answer, expressed as depth from the
/// bottom of the screen (depth 0 = bottom row).
///
/// The cursor sits inside the input box. Everything below it is frame, not
/// answer, so we align the starting count with the cursor row. Since this
/// works on position rather than text content, it's never fooled even if the
/// answer's wording happens to coincide with the frame's
pub fn capture_range(rows: u16, cursor_row: u16, since: usize) -> (usize, usize) {
    let below = rows.saturating_sub(1).saturating_sub(cursor_row) as usize;
    // We add `below` so the range reaches the target row. Once it does, we don't pad further
    (below, below.saturating_add(since))
}

/// Turn a row range within the scrollback (row count from the bottom of the
/// screen, lo..=hi) into text.
/// Wrapped rows are joined together, and trailing whitespace is trimmed.
/// One line of history, by its distance from the newest line (0 = newest).
///
/// Reading history means moving the screen's own viewport, which is why this
/// and `extract_text` both put it back afterwards: the viewport is what the
/// person is looking at, not a cursor we own
fn line_at<CB: vt100::Callbacks>(p: &mut vt100::Parser<CB>, d: usize, cols: u16) -> String {
    let max = furthest_back(p);
    let s = d.min(max);
    let (rows, _) = p.screen().size();
    p.screen_mut().set_scrollback(s);
    let r = (rows as usize).saturating_sub(1).saturating_sub(d - s);
    p.screen().rows(0, cols).nth(r).unwrap_or_default()
}

/// How far back the history goes, in lines
fn furthest_back<CB: vt100::Callbacks>(p: &mut vt100::Parser<CB>) -> usize {
    let saved = p.screen().scrollback();
    p.screen_mut().set_scrollback(usize::MAX / 2);
    let max = p.screen().scrollback();
    p.screen_mut().set_scrollback(saved);
    max
}

/// The nearest line containing `needle`, starting from `from` and moving in
/// one direction, then wrapping once.
///
/// Wrapping is what makes one key enough. Copy mode opens at the newest line,
/// where searching "down" has nothing below it to find — a search that
/// answered "no matches" there would be technically right and useless. So it
/// looks the other way rather than stopping, and either way it stops before
/// coming back to where it began.
pub fn find_line<CB: vt100::Callbacks>(
    p: &mut vt100::Parser<CB>,
    needle: &str,
    from: usize,
    up: bool,
    cols: u16,
) -> Option<usize> {
    let needle = needle.to_lowercase();
    if needle.is_empty() {
        return None;
    }
    let saved = p.screen().scrollback();
    let (rows, _) = p.screen().size();
    let top = furthest_back(p) + (rows as usize).saturating_sub(1);
    // Every line exactly once, starting next to where we are and carrying on
    // past either end
    let order: Vec<usize> = match up {
        true => (0..=top).map(|i| (from + 1 + i) % (top + 1)).collect(),
        false => (0..=top)
            .map(|i| (from + top + 1 - (i % (top + 1))) % (top + 1))
            .collect(),
    };
    let hit = order
        .into_iter()
        .find(|d| line_at(p, *d, cols).to_lowercase().contains(&needle));
    p.screen_mut().set_scrollback(saved);
    hit
}

pub fn extract_text<CB: vt100::Callbacks>(
    p: &mut vt100::Parser<CB>,
    lo: usize,
    hi: usize,
    cols: u16,
) -> String {
    let saved = p.screen().scrollback();
    p.screen_mut().set_scrollback(usize::MAX / 2);
    let max = p.screen().scrollback();
    let (rows, _) = p.screen().size();
    let top = max + rows.saturating_sub(1) as usize;
    let mut out = String::new();
    for d in (lo..=hi.min(top)).rev() {
        let s = d.min(max);
        p.screen_mut().set_scrollback(s);
        let r = (rows as usize - 1 - (d - s)) as u16;
        // rows(start_col, width) is an iterator returning a horizontal slice of each visible row
        let line = p
            .screen()
            .rows(0, cols)
            .nth(r as usize)
            .unwrap_or_default();
        if p.screen().row_wrapped(r) {
            out.push_str(&line);
        } else {
            out.push_str(line.trim_end());
            out.push('\n');
        }
    }
    p.screen_mut().set_scrollback(saved);
    out
}

/// Does this input contain a "submit"?
///
/// The contents of a bracketed paste are just body text, so a newline inside
/// it is not a submit. If typing or pasting alone counted as submitting, a
/// screen that simply went idle mid-typing would be misread as a completed
/// response
pub fn contains_submit(bytes: &[u8]) -> bool {
    const START: &[u8] = b"\x1b[200~";
    const END: &[u8] = b"\x1b[201~";
    let mut in_paste = false;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i..].starts_with(START) {
            in_paste = true;
            i += START.len();
        } else if bytes[i..].starts_with(END) {
            in_paste = false;
            i += END.len();
        } else {
            if !in_paste && matches!(bytes[i], b'\r' | b'\n') {
                return true;
            }
            i += 1;
        }
    }
    false
}

/// Turn the screen into text exactly as displayed (1 screen row = 1 line).
///
/// `Screen::contents()` joins rows treated as wrapped without a newline.
/// That's correct as plain text, but ASCII art that draws all the way to the
/// edge of every line gets treated as wrapped on every row, collapsing the
/// entire screen into a single line.
/// When the goal is to carry the visual layout, rows need to be preserved
pub fn visible_text(screen: &vt100::Screen) -> String {
    let (_, cols) = screen.size();
    screen
        .rows(0, cols)
        .map(|l| l.trim_end().to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Hash of the screen content. The bottom `ignore_bottom` rows are excluded
/// from the judgment
/// (status bars like byobu/tmux update their clock every second, and if we
///  looked at raw output activity the tab would look BUSY forever)
pub fn screen_hash(screen: &vt100::Screen, ignore_bottom: u16) -> u64 {
    use std::hash::{Hash as _, Hasher as _};
    let (rows, cols) = screen.size();
    let keep = rows.saturating_sub(ignore_bottom) as usize;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for line in screen.rows(0, cols).take(keep) {
        line.hash(&mut h);
    }
    h.finish()
}

#[cfg(test)]
mod changed_span_tests {
    use super::Tab;

    fn rows(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// In a full-screen TUI, anything that was already on screen before
    /// execution must not be included in the answer.
    ///
    /// Claude Code doesn't scroll, so "rows written since execution" couldn't
    /// be counted, and the whole visible screen was captured instead. As a
    /// result, the startup banner and the input box frame were being sent to
    /// the other AI too
    #[test]
    fn what_was_already_on_screen_is_not_the_answer() {
        let before = rows(&[
            "  Claude Code v2.1.223", // startup banner
            "  D:\\project",
            "",
            "────────",             // top edge of frame
            "> 質問",
            "────────",             // bottom edge of frame
            "  ? for shortcuts",
        ]);
        let now = rows(&[
            "  Claude Code v2.1.223", // unchanged
            "  D:\\project",
            "",
            "答えの1行目", // changed starting here
            "答えの2行目",
            "────────", // unchanged again (back to the frame)
            "  ? for shortcuts",
        ]);
        assert_eq!(
            Tab::changed_span(&before, &now),
            (3, 5),
            "バナーと枠を外し、変わった2行だけを残す"
        );
    }

    /// If no answer has arrived yet, nothing should be returned
    #[test]
    fn an_unchanged_screen_yields_nothing() {
        let same = rows(&["a", "b", "c"]);
        assert_eq!(Tab::changed_span(&same, &same), (0, 0));
    }

    /// Trim only the edges, and don't touch the middle.
    ///
    /// Even if a row identical to a pre-execution row happens to appear in
    /// the middle of the answer, it must not be cut there
    #[test]
    fn a_coincidence_in_the_middle_does_not_split_the_answer() {
        let before = rows(&["枠", "x", "同じ行", "y", "枠"]);
        let now = rows(&["枠", "答え1", "同じ行", "答え2", "枠"]);
        assert_eq!(
            Tab::changed_span(&before, &now),
            (1, 4),
            "真ん中の一致では切らない"
        );
    }

    /// If the row count changed, the bottom edges no longer correspond, so decide from the top edge alone
    #[test]
    fn a_resized_screen_falls_back_to_the_top_edge() {
        let before = rows(&["枠", "x"]);
        let now = rows(&["枠", "答え", "増えた行"]);
        assert_eq!(Tab::changed_span(&before, &now), (1, 3));
    }

    /// If we never captured the pre-execution screen, don't trim anything (fail safe)
    #[test]
    fn without_a_snapshot_nothing_is_removed() {
        let now = rows(&["a", "b", "c"]);
        assert_eq!(Tab::changed_span(&[], &now), (0, 3));
    }
}

#[cfg(test)]
mod capture_range_tests {
    use super::capture_range;

    /// Anything below the input box must not be passed along as part of the answer.
    ///
    /// What the user saw was 'Use /skills to list available skills' and
    /// 'gpt-5.5 medium  D:\\Test' being forwarded to the other side. Both are
    /// frame drawn below the cursor, not the answer.
    ///
    /// Cut by position, not by text. That way it's never fooled even if the
    /// answer's wording happens to coincide with the frame's, and it doesn't
    /// break when the CLI changes its wording
    #[test]
    fn the_frame_below_the_cursor_is_not_part_of_the_answer() {
        // A 24-row screen; the cursor sits in the input box (4th row from the
        // bottom). The 3 rows below it are a hint row and a status row
        let (lo, hi) = capture_range(24, 20, 10);
        assert_eq!(lo, 3, "カーソルより下の3行を飛ばして数え始める");
        assert_eq!(hi, 13, "実行してから書かれた10行ぶんを取る");

        // Plain shell: the cursor sits at the prompt on the bottom row
        let (lo, hi) = capture_range(24, 23, 5);
        assert_eq!((lo, hi), (0, 5), "下に枠がなければ最下行から数える");

        // If nothing was written, nothing is taken (lo == hi gives 1 row, but
        // that row is the cursor row itself = the input box, so trim drops it)
        let (lo, hi) = capture_range(24, 20, 0);
        assert_eq!(lo, hi, "実行後に何も書かれていなければ範囲は空に近い");
    }

    /// Even with a broken screen size, the range calculation alone must not break down
    #[test]
    fn a_broken_screen_size_does_not_panic() {
        assert_eq!(capture_range(0, 0, 0), (0, 0), "高さ0");
        assert_eq!(capture_range(1, 5, 3), (0, 3), "カーソルが画面外");
        let (lo, hi) = capture_range(24, 0, usize::MAX);
        assert_eq!(lo, 23);
        assert_eq!(hi, usize::MAX, "足し算が溢れない");
    }
}

#[cfg(test)]
mod tests {
    use super::screen_hash;

    fn argv(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    fn spec(new_id: &[&str], with_id: &[&str], newest: &[&str]) -> crate::profile::ResumeSpec {
        crate::profile::ResumeSpec {
            new_id: new_id.iter().map(|s| s.to_string()).collect(),
            with_id: with_id.iter().map(|s| s.to_string()).collect(),
            newest_here: newest.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn searching_the_history_wraps_instead_of_giving_up() {
        // Copy mode opens at the newest line. A search that only looked
        // "down" from there would have nothing below it to find and would
        // answer "no matches" while the thing sits plainly above
        let mut p = vt100::Parser::new(4, 20, 200);
        for line in ["alpha", "beta", "NEEDLE here", "gamma", "delta", "epsilon"] {
            p.process(format!("{line}
").as_bytes());
        }
        let found = super::find_line(&mut p, "needle", 0, true, 20).expect("見つかる");
        let text = super::extract_text(&mut p, found, found, 20);
        assert!(text.contains("NEEDLE here"), "掴んだ行が違う: {text:?}");
        // Case is not what anyone means when they search a terminal
        assert_eq!(super::find_line(&mut p, "NeEdLe", 0, true, 20), Some(found));
        // The other direction reaches it too, by wrapping past the newest line
        assert_eq!(super::find_line(&mut p, "needle", 0, false, 20), Some(found));
        // Nothing there is nothing there
        assert_eq!(super::find_line(&mut p, "haystack", 0, true, 20), None);
        // ...and an empty search is not a search
        assert_eq!(super::find_line(&mut p, "", 0, true, 20), None);
        // Starting ON the match moves off it rather than standing still, so
        // pressing "next" repeatedly walks rather than sticks
        let again = super::find_line(&mut p, "needle", found, true, 20);
        assert_eq!(again, Some(found), "他に無ければ一周して同じ行に戻る");
    }

    #[test]
    fn every_spelling_of_a_terminal_notification_is_understood() {
        let p = |parts: &[&str]| -> Option<(String, String)> {
            let owned: Vec<Vec<u8>> = parts.iter().map(|s| s.as_bytes().to_vec()).collect();
            let refs: Vec<&[u8]> = owned.iter().map(|v| v.as_slice()).collect();
            super::note_of(&refs)
        };
        // The oldest one carries a body and nothing else
        assert_eq!(p(&["9", "build finished"]), Some((String::new(), "build finished".into())));
        // The common one carries both
        assert_eq!(
            p(&["777", "notify", "Build", "3 tests failed"]),
            Some(("Build".into(), "3 tests failed".into()))
        );
        // A body with a semicolon in it survives being split on semicolons
        assert_eq!(
            p(&["777", "notify", "Build", "failed", "then retried"]),
            Some(("Build".into(), "failed;then retried".into()))
        );
        // The rich one puts metadata first, and says when the text is a title
        assert_eq!(p(&["99", "i=1:d=0:Hello"]), Some((String::new(), "Hello".into())));
        assert_eq!(p(&["99", "i=1:p=title:Done"]), Some(("Done".into(), String::new())));
        // Everything else on the wire is somebody else's business
        assert_eq!(p(&["0", "a window title"]), None);
        assert_eq!(p(&["777", "something-else", "x"]), None);
        assert_eq!(p(&["99", "i=1:d=0:"]), None);
    }

    #[test]
    fn a_fresh_launch_hands_over_an_id_of_ours_when_the_cli_takes_one() {
        // The strongest form: two tabs running the same CLI in the same folder
        // are told apart because each was given its own conversation to start
        let s = spec(&["--session-id", "{id}"], &["--resume", "{id}"], &[]);
        let (out, session) = super::plan_launch(Some(&s), &argv("claude"), super::Resume::Fresh);
        let session = session.expect("a conversation was started");
        assert_eq!(session.source, super::SessionSource::Minted);
        assert_eq!(out, vec!["claude", "--session-id", &session.id]);
        assert_eq!(session.id.len(), 36, "UUIDの綴りで渡す: {}", session.id);
        assert_eq!(&session.id[14..15], "4", "version 4 と名乗る");
    }

    #[test]
    fn resuming_puts_the_arguments_straight_after_the_program() {
        // codex resumes by SUBCOMMAND, so "codex --search resume <id>" would be
        // read as flags to a subcommand that hasn't been named yet
        let s = spec(&[], &["resume", "{id}"], &[]);
        let was = super::Session {
            id: "0198-abc".into(),
            source: super::SessionSource::Hook,
        };
        let (out, session) =
            super::plan_launch(Some(&s), &argv("codex --search"), super::Resume::Id(was.clone()));
        assert_eq!(out, vec!["codex", "resume", "0198-abc", "--search"]);
        assert_eq!(session, Some(was), "引き継いだ会話をそのまま覚えている");
    }

    #[test]
    fn a_cli_with_nothing_to_resume_is_launched_exactly_as_written() {
        let (out, session) = super::plan_launch(None, &argv("aider --model x"), super::Resume::Fresh);
        assert_eq!(out, argv("aider --model x"));
        assert_eq!(session, None);
        // ...and asking to carry a conversation over changes nothing either
        let s = spec(&[], &[], &[]);
        let (out, _) = super::plan_launch(Some(&s), &argv("aider"), super::Resume::NewestHere);
        assert_eq!(out, argv("aider"));
    }

    #[test]
    fn a_command_that_already_resumes_is_left_exactly_as_written() {
        // "claude --dangerously-skip-permissions --resume" plus our own
        // "--session-id <uuid>" is a line Claude Code refuses to start:
        // --session-id may not join --resume without --fork-session. The tab
        // died on every restart, because every restart wrote the same line
        let s = spec(&["--session-id", "{id}"], &["--resume", "{id}"], &["--continue"]);
        let written = argv("claude --dangerously-skip-permissions --resume");
        let (out, session) = super::plan_launch(Some(&s), &written, super::Resume::Fresh);
        assert_eq!(out, written, "自分で書いた再開の指定に、こちらの指定を重ねない");
        assert_eq!(session, None, "こちらが選んでいない会話を、覚えたことにしない");
        // --continue is the same story, and so is a resume spelled as a
        // subcommand
        let (out, _) =
            super::plan_launch(Some(&s), &argv("claude --continue"), super::Resume::Fresh);
        assert_eq!(out, argv("claude --continue"));
        let c = spec(&[], &["resume", "{id}"], &["resume", "--last"]);
        let (out, _) = super::plan_launch(Some(&c), &argv("codex resume"), super::Resume::Fresh);
        assert_eq!(out, argv("codex resume"));
        // The program's own name is not one of those words: "codex" alone
        // still gets everything it always got
        let (out, session) = super::plan_launch(Some(&c), &argv("codex"), super::Resume::NewestHere);
        assert_eq!(out, argv("codex resume --last"));
        assert_eq!(session, None);
    }

    #[test]
    fn the_settings_screen_is_shown_the_line_that_will_actually_run() {
        // Read against the profiles this repo ships, on purpose: the promise
        // is about what really starts, so a stub here would prove nothing
        let line = super::launch_line(&argv("claude"), &None, super::Resume::Fresh, "<new>");
        assert_eq!(line.argv, argv("claude --session-id <new>"));
        assert_eq!(line.added, 2, "アプリが足した語数を数えている");
        // The id is where it comes from, not a number: redrawing the field
        // must not show a different conversation every time
        let again = super::launch_line(&argv("claude"), &None, super::Resume::Fresh, "<new>");
        assert_eq!(again.argv, line.argv, "描き直すたびに違う番号を見せない");
        // Nothing added, nothing to highlight
        let plain = super::launch_line(&argv("powershell.exe"), &None, super::Resume::Fresh, "<x>");
        assert_eq!(plain.argv, argv("powershell.exe"));
        assert_eq!(plain.added, 0);
        // And the tab this all started with: written as it was, run as it was
        let written = argv("claude --dangerously-skip-permissions --resume");
        let kept = super::launch_line(&written, &None, super::Resume::Fresh, "<new>");
        assert_eq!(kept.argv, written);
        assert_eq!(kept.added, 0);
    }

    #[test]
    fn continuing_the_newest_here_names_no_conversation() {
        // The CLI picks it, so afterwards we do not know which one it picked —
        // and saying we do would be a lie the next restart would act on
        let s = spec(&[], &[], &["--continue"]);
        let (out, session) = super::plan_launch(Some(&s), &argv("claude"), super::Resume::NewestHere);
        assert_eq!(out, vec!["claude", "--continue"]);
        assert_eq!(session, None);
    }

    #[test]
    fn auto_runs_needs_the_bypass_flag_for_a_cli() {
        // A bare CLI still asks for confirmation, so it can't drive a tab...
        assert!(!super::argv_auto_runs(&argv("claude"), false));
        assert!(!super::argv_auto_runs(&argv("codex --model o3"), false));
        // ...but with its own bypass flag it runs unattended.
        assert!(super::argv_auto_runs(&argv("claude --dangerously-skip-permissions"), false));
        assert!(super::argv_auto_runs(
            &argv("codex --dangerously-bypass-approvals-and-sandbox"),
            false
        ));
        assert!(super::argv_auto_runs(&argv("gemini --yolo"), false));
        // A path-qualified head still resolves (file_stem), and flag order is free.
        assert!(super::argv_auto_runs(
            &argv("/usr/bin/claude --foo --dangerously-skip-permissions"),
            false
        ));
    }

    #[test]
    fn auto_runs_is_true_for_a_model_and_false_for_others() {
        // A model bridge writes replies in-process — no flag, always autonomous.
        assert!(super::argv_auto_runs(&argv("anything"), true));
        // Shells and CLIs without a known bypass flag never auto-run.
        assert!(!super::argv_auto_runs(&argv("cmd.exe"), false));
        assert!(!super::argv_auto_runs(&argv("aider --yes"), false));
        assert!(!super::argv_auto_runs(&[], false));
    }

    /// A portable build meeting a PC without the tool installed should get a
    /// plain-language pointer to Settings, not the raw CreateProcessW error.
    #[test]
    fn a_missing_command_is_explained_not_dumped() {
        let msg = super::launch_problem(
            "GEMINI",
            "shikisha-not-a-real-program-xyz",
            None,
            "CreateProcessW `\"x\\0\"` failed: os error 2",
        );
        assert!(msg.contains("GEMINI"), "names the tab: {msg}");
        assert!(msg.contains("shikisha-not-a-real-program-xyz"), "names the command: {msg}");
        assert!(!msg.contains("CreateProcessW"), "no raw error leaks: {msg}");
        assert!(!msg.contains("os error"), "no raw error leaks: {msg}");
    }

    /// A missing working folder is diagnosed before the command, since both
    /// come back from the OS as the same "file not found".
    #[test]
    fn a_missing_folder_wins_over_an_installed_command() {
        let missing = std::path::Path::new("Z:/shikisha/no/such/folder");
        // cmd.exe is present, so only the missing folder can produce this message
        let msg = super::launch_problem("SHELL", "cmd.exe", Some(missing), "os error 2");
        assert!(msg.contains("SHELL"), "names the tab: {msg}");
        assert!(msg.contains("Z:") && msg.contains("folder"), "explains the folder: {msg}");
        assert!(!msg.contains("os error"), "no raw error leaks: {msg}");
    }

    /// A row drawn all the way to the edge of the screen must not collapse
    /// into a single line.
    /// contents() joins wrapped rows together, so ASCII art ends up all
    /// concatenated into a display with "no line breaks" (this actually happened)
    #[test]
    fn full_width_rows_keep_their_line_breaks() {
        let mut p = vt100::Parser::new(4, 10, 0);
        // Three rows of exactly 10 columns. The terminal records these as wrapped
        p.process(b"##########$$$$$$$$$$%%%%%%%%%%");

        assert!(
            !p.screen().contents().contains('\n'),
            "contents() は改行を落とす (この前提が崩れたら本関数は不要)"
        );
        let visible = super::visible_text(p.screen());
        assert_eq!(
            visible.split('\n').collect::<Vec<_>>(),
            vec!["##########", "$$$$$$$$$$", "%%%%%%%%%%", ""],
            "画面の行がそのまま残る (4行目は空行)"
        );
    }

    /// When the submit didn't actually take effect, that must not be called a response.
    ///
    /// Even a paste redrawing into the `[Pasted Content …]` form increases
    /// output and moves the screen. If that's treated as the signal for a
    /// response, the ball gets passed even though the submit never landed.
    /// Judge it by whether the "AI started working" display appeared
    #[test]
    fn an_answer_requires_the_ai_to_have_started_working() {
        use super::{Tab, TabOptions};
        use std::sync::atomic::Ordering;
        use std::time::{Duration, Instant};

        // Pick a profile that has a "working" indicator
        let argv = vec!["cmd.exe".to_string()];
        let mut t = Tab::spawn(
            "shell".into(),
            &argv,
            Some("claude".into()),
            12,
            60,
            TabOptions::default(),
        )
        .unwrap();
        let start = Instant::now();
        for _ in 0..40 {
            std::thread::sleep(Duration::from_millis(50));
            t.tick(start);
        }

        t.write_bytes(b"echo REPLY\r").unwrap();
        assert!(t.was_prompted(), "実行として記録される");
        assert!(!t.answered_since_submit(), "実行した直後はまだ応答が無い");

        // Output moved and the screen changed, but the AI hasn't started working (paste redraw)
        for _ in 0..40 {
            std::thread::sleep(Duration::from_millis(50));
            t.tick(start);
        }
        assert!(t.output_count() > 0 && t.had_output(), "出力そのものは動いている");
        assert!(
            !t.answered_since_submit(),
            "画面が動いただけで応答ありと数えている (実行が効いていなくてもボールが渡る)"
        );

        // Once the "started working" indicator is seen, it counts as answered
        t.saw_working.store(true, Ordering::Relaxed);
        assert!(t.answered_since_submit(), "働き始めたら応答として数える");

        // The next submit resets it back to a waiting state
        t.write_bytes(b"echo AGAIN\r").unwrap();
        assert!(
            !t.answered_since_submit(),
            "実行のたびに数え直す (前の応答が残らない)"
        );

        t.kill();
    }

    /// A working indicator that was never observed must not turn a real
    /// answer into a permanent "no response" — CLIs restyle their status
    /// line, and a short turn can finish between polls. Output that keeps
    /// moving well after the submit is accepted as the secondary evidence
    /// (the submit's own echo can't fake it: that burst ends immediately).
    #[test]
    fn sustained_output_after_submit_counts_as_answered() {
        use super::{Tab, TabOptions};
        use std::time::{Duration, Instant};

        let argv = vec!["cmd.exe".to_string()];
        let mut t = Tab::spawn(
            "shell".into(),
            &argv,
            Some("claude".into()),
            12,
            60,
            TabOptions::default(),
        )
        .unwrap();
        let start = Instant::now();
        for _ in 0..40 {
            std::thread::sleep(Duration::from_millis(50));
            t.tick(start);
        }

        t.write_bytes(b"echo REPLY\r").unwrap();
        // The echo burst lands right away, then goes quiet: even after the
        // grace window it must not count as an answer on its own
        for _ in 0..50 {
            std::thread::sleep(Duration::from_millis(50));
            t.tick(start);
        }
        assert!(
            !t.answered_since_submit(),
            "エコーだけ (直後の一瞬で止まった出力) は応答にしない"
        );

        // Output arriving well after the submit = the peer actually said
        // something, indicator or not
        t.write_passthrough(b"echo LATE-REPLY\r").unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && !t.answered_since_submit() {
            std::thread::sleep(Duration::from_millis(50));
            t.tick(start);
        }
        assert!(
            t.answered_since_submit(),
            "働き表示を見逃しても、submitのずっと後に動いた出力で応答と分かる"
        );

        t.kill();
    }

    /// The start of a response must be fixed at "the moment execution was submitted."
    ///
    /// If it were "the first position where the screen moved" instead, a
    /// paste being displayed or the input box redrawing would also move the
    /// screen, so the frame would be grabbed instead of the answer (this
    /// actually happened)
    #[test]
    fn a_response_starts_where_the_instruction_was_submitted() {
        use super::{Tab, TabOptions};
        use std::sync::atomic::Ordering;
        use std::time::{Duration, Instant};

        let argv = vec!["cmd.exe".to_string()];
        let mut t = Tab::spawn("shell".into(), &argv, None, 10, 60, TabOptions::default()).unwrap();
        let start = Instant::now();
        let marker = |t: &Tab| t.response_marker.load(Ordering::Relaxed);
        for _ in 0..40 {
            std::thread::sleep(Duration::from_millis(50));
            t.tick(start);
        }
        assert_eq!(marker(&t), u64::MAX, "実行していないうちは始まりが無い");

        // Fixed at the moment of execution (doesn't wait for the screen to move)
        t.write_bytes(b"echo ONE\r").unwrap();
        let began = marker(&t);
        assert_ne!(began, u64::MAX, "実行した瞬間に始まりが決まる");

        // No matter how much the screen moves after that, it's not re-taken
        for _ in 0..40 {
            std::thread::sleep(Duration::from_millis(50));
            t.tick(start);
        }
        assert_eq!(marker(&t), began, "画面が動いても始まりは動かない");

        // Once fully received, wait for the next execution
        t.finish_response();
        assert_eq!(marker(&t), u64::MAX, "次の応答は新しく取り直す");
        assert!(!t.was_prompted(), "次の実行を待つ状態に戻る");

        t.kill();
    }

    /// Just typing or just pasting must not count as "submitted."
    ///
    /// If it did, a screen that went idle mid-typing would be misread as a
    /// completed response, and the half-written content would be forwarded
    /// to other tabs
    #[test]
    fn only_a_real_enter_counts_as_submitting() {
        use super::contains_submit;

        assert!(!contains_submit(b"hello"), "打っただけ");
        assert!(!contains_submit(b""), "空");
        assert!(contains_submit(b"hello\r"), "改行で実行");
        assert!(contains_submit(b"\r"), "改行だけでも実行");
        assert!(contains_submit(b"\n"), "LFも実行として扱う");

        // The contents of a bracketed paste are body text. A newline inside it is not a submit
        assert!(
            !contains_submit(b"\x1b[200~one\rtwo\x1b[201~"),
            "貼り付けた本文の改行は実行ではない"
        );
        // A newline after the paste closes is a submit
        assert!(
            contains_submit(b"\x1b[200~one\rtwo\x1b[201~\r"),
            "貼り付けを閉じたあとの改行は実行"
        );
        // Even if the paste is never closed, its contents are not mistaken for a submit
        assert!(
            !contains_submit(b"\x1b[200~one\rtwo"),
            "閉じられていない貼り付けの中身"
        );
    }

    /// Merely resizing the screen must not be treated as "a response arrived."
    ///
    /// A child process redraws the screen when the terminal size changes.
    /// The content is the same but the screen still moves, so counting that
    /// as activity would walk through BUSY→DONE and forward the redraw to
    /// other tabs as if it were a new response
    #[test]
    fn resizing_the_window_is_not_a_new_answer() {
        use super::{Tab, TabOptions};
        use crate::detect::TabState;
        use std::time::{Duration, Instant};

        let argv = vec!["cmd.exe".to_string()];
        let mut t = Tab::spawn("shell".into(), &argv, None, 20, 60, TabOptions::default()).unwrap();
        let start = Instant::now();

        // After startup, run until it settles down
        let settle = |t: &mut Tab| {
            for _ in 0..120 {
                std::thread::sleep(Duration::from_millis(50));
                if t.tick(start).1 != TabState::Busy {
                    return t.state;
                }
            }
            t.state
        };
        let calm = settle(&mut t);
        assert_ne!(calm, TabState::Busy, "まず落ち着かせる");

        // Change the size. The child process redraws, but it's not a response
        t.resize(30, 100).unwrap();
        let mut went_busy = false;
        for _ in 0..30 {
            std::thread::sleep(Duration::from_millis(50));
            if t.tick(start).1 == TabState::Busy {
                went_busy = true;
                break;
            }
        }
        assert!(
            !went_busy,
            "描き直しを処理中と見なしている (このあと DONE になり応答として転送される)"
        );

        t.kill();
    }

    /// Startup output alone reaches DONE, and that must not be treated as an answer.
    ///
    /// Any program outputs something at startup, so the screen goes through
    /// "moves → stops" and the state necessarily becomes DONE. If this were
    /// treated as a completed response, a banner nobody asked for would get
    /// forwarded to other tabs by automation
    #[test]
    fn startup_output_reaches_done_but_is_not_an_answer() {
        use super::{Tab, TabOptions};
        use crate::detect::TabState;
        use std::time::{Duration, Instant};

        let argv = vec!["cmd.exe".to_string()];
        let mut t = Tab::spawn("shell".into(), &argv, None, 20, 60, TabOptions::default()).unwrap();

        // Confirm that startup output alone reaches DONE (checking the premise)
        let start = Instant::now();
        let mut saw_done = false;
        for _ in 0..120 {
            std::thread::sleep(Duration::from_millis(50));
            if t.tick(start).1 == TabState::Done {
                saw_done = true;
                break;
            }
        }
        assert!(saw_done, "起動しただけで DONE になる");

        // Nobody submitted any input, so this must not be treated as a response
        assert!(
            !t.was_prompted(),
            "何も聞いていないのに応答完了として扱われている"
        );

        // A DONE that comes after real input is a genuine response
        t.write_bytes(b"echo hi\r").unwrap();
        assert!(t.was_prompted(), "入力したら応答を待つ状態になる");

        t.kill();
    }

    /// Automation right after startup must wait until the program can actually accept input.
    ///
    /// An AI CLI discards input until it finishes drawing the input box after
    /// launch. Sending input immediately means the configured text goes
    /// nowhere (this actually happened)
    #[test]
    fn the_startup_hook_waits_until_the_program_settles() {
        use super::{Tab, TabOptions};
        use std::time::{Duration, Instant};

        let argv = vec!["cmd.exe".to_string()];
        let mut t = Tab::spawn("shell".into(), &argv, None, 20, 60, TabOptions::default()).unwrap();

        // Nothing has been output yet = still starting up, so don't send input
        let start = Instant::now();
        assert!(!t.had_output(), "起動直後は無出力");
        assert!(
            !t.ready_for_startup_hook(start.elapsed().as_millis() as u64),
            "無出力のうちは待つ"
        );

        // Once output appears and the screen settles, it's ready
        let mut became_ready = false;
        let mut ready_at = 0;
        let mut first_output_at = None;
        for _ in 0..120 {
            std::thread::sleep(Duration::from_millis(50));
            let now = start.elapsed().as_millis() as u64;
            t.tick(start);
            if first_output_at.is_none() && t.had_output() {
                first_output_at = Some(now);
            }
            if t.ready_for_startup_hook(now) {
                became_ready = true;
                ready_at = now;
                break;
            }
        }
        assert!(became_ready, "落ち着いたら準備完了になる");
        assert!(t.had_output(), "出力が出たことを根拠にしている");
        assert!(
            t.age_ms() < 15_000,
            "時間切れではなく、落ち着いたことで判定できている ({}ms)",
            t.age_ms()
        );
        // The point of the whole gate: the first byte is not the invitation to
        // type. Something has to hold still afterwards. Without this the tab
        // was declared ready on the same tick its banner appeared, and the
        // persona went into a CLI that had not drawn its input box yet.
        let out_at = first_output_at.expect("出力が出たのに記録されていない");
        assert!(
            ready_at >= out_at + 500,
            "出力が出た瞬間に準備完了にしている (出力 {out_at}ms → 準備完了 {ready_at}ms)"
        );

        t.kill();
    }

    #[test]
    fn bottom_status_rows_are_ignored() {
        let mut p = vt100::Parser::new(5, 20, 0);
        p.process(b"main content\r\n");
        let before = screen_hash(p.screen(), 2);
        // Rewrite only the bottom row (the equivalent of byobu's clock)
        p.process(b"\x1b[5;1H12:34:56");
        assert_eq!(before, screen_hash(p.screen(), 2), "最下部の変化は無視");
        // The hash changes if the body content changes
        p.process(b"\x1b[1;1Hchanged!");
        assert_ne!(before, screen_hash(p.screen(), 2));
    }
}

/// The arguments a launch really runs with, and the conversation it will carry.
///
/// `{id}` is the only thing substituted, and the arguments go **straight after
/// the program**: a CLI that resumes by subcommand needs them there, and one
/// that resumes by flag does not mind. The program itself is never named by a
/// profile, so no profile can point a tab at something else to run — which is
/// what makes running this unattended safe without anyone approving anything.
fn plan_launch(
    spec: Option<&crate::profile::ResumeSpec>,
    argv: &[String],
    plan: Resume,
) -> (Vec<String>, Option<Session>) {
    let Some(spec) = spec else {
        return (argv.to_vec(), None);
    };
    // A command someone wrote themselves may already say how to resume. Ours
    // on top of theirs names two conversations in one line, and the CLI
    // refuses to start at all -- Claude Code answers "--session-id can only be
    // used with --continue or --resume if --fork-session is also specified",
    // and because a restart rebuilds the same line the tab stays dead however
    // many times it is restarted. Whoever wrote those words meant them; we
    // stand aside, and claim no conversation we did not choose
    if already_resumes(spec, argv) {
        return (argv.to_vec(), None);
    }
    let put = |extra: &[String], id: &str| -> Vec<String> {
        let mut out = argv.to_vec();
        let at = 1.min(out.len());
        for (i, a) in extra.iter().enumerate() {
            out.insert(at + i, a.replace("{id}", id));
        }
        out
    };
    match plan {
        Resume::Id(s) if !spec.with_id.is_empty() => (put(&spec.with_id, &s.id), Some(s)),
        // Asked to carry one on by a CLI that cannot be told which: the caller
        // only offers this after checking nobody else could have been here
        Resume::NewestHere if !spec.newest_here.is_empty() => (put(&spec.newest_here, ""), None),
        // A new conversation. Where the CLI accepts an id, ours is the one it
        // gets, and there is nothing left to work out later
        _ if !spec.new_id.is_empty() => {
            let id = crate::random_uuid();
            let out = put(&spec.new_id, &id);
            (out, Some(Session { id, source: SessionSource::Minted }))
        }
        _ => (argv.to_vec(), None),
    }
}

/// The command line a launch will really run, written out for a person to read.
///
/// A command field that shows one line while another one starts is how an
/// argument nobody typed stays invisible until a CLI refuses it. This is the
/// same assembly the launch itself performs -- `plan_launch` is called, never
/// imitated -- so what the settings screen promises and what actually starts
/// cannot drift apart.
///
/// An id we would mint is shown as `minted_as` rather than as a number: the
/// number does not exist until the launch, and printing a different one each
/// time the field is redrawn would teach nobody anything true.
pub struct LaunchLine {
    /// Every argument, program first, in the order it will be passed
    pub argv: Vec<String>,
    /// How many of them this app put there. They sit at `argv[1..1 + added]`,
    /// because `plan_launch` inserts them in one run right after the program
    pub added: usize,
}

pub fn launch_line(
    argv: &[String],
    profile_spec: &Option<String>,
    plan: Resume,
    minted_as: &str,
) -> LaunchLine {
    let spec = Tab::resolve_profile(argv, profile_spec).resume;
    let (mut out, session) = plan_launch(spec.as_ref(), argv, plan);
    let added = out.len().saturating_sub(argv.len());
    if let Some(s) = session.filter(|s| s.source == SessionSource::Minted) {
        for a in out.iter_mut().skip(1).take(added) {
            *a = a.replace(&s.id, minted_as);
        }
    }
    LaunchLine { argv: out, added }
}

/// Whether the command as written already asks this CLI to resume something.
///
/// The words are taken from the profile's own templates rather than a list
/// kept here, so a CLI that spells it `resume` as a subcommand is recognised
/// by the same rule as one that spells it `--continue`. `{id}` slots are not
/// words anybody types, so they are left out; the program itself is skipped,
/// or `codex` would look like the `resume` in its own template.
fn already_resumes(spec: &crate::profile::ResumeSpec, argv: &[String]) -> bool {
    let mut words = spec
        .new_id
        .iter()
        .chain(&spec.with_id)
        .chain(&spec.newest_here)
        .filter(|w| !w.contains("{id}"));
    words.any(|w| argv.iter().skip(1).any(|given| given == w))
}

/// Fingerprint of the launch conditions. If this changes, a new session must be created to take effect
pub fn signature_of(argv: &[String], opts: &TabOptions) -> String {
    format!(
        "{}|{}|{}|{}",
        argv.join(" "),
        opts.encoding.map(|e| e.name()).unwrap_or("UTF-8"),
        opts.scrollback,
        opts.cwd
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default()
    )
}

/// The "act without asking" flag a CLI needs to run unattended, or None if it has
/// none. Single source of truth for the operator-readiness gate; the settings JS
/// (webui `cliFlagOf`) mirrors these strings for the editable checkbox.
fn bypass_flag(head: &str) -> Option<&'static str> {
    match head {
        "claude" => Some("--dangerously-skip-permissions"),
        "codex" => Some("--dangerously-bypass-approvals-and-sandbox"),
        "gemini" => Some("--yolo"),
        _ => None,
    }
}

/// Whether a launch (its argv, plus whether it's a model bridge) acts without
/// per-action confirmation. A model always does; a CLI only with its bypass flag
/// present; everything else never does.
pub fn argv_auto_runs(argv: &[String], is_model: bool) -> bool {
    if is_model {
        return true;
    }
    let Some(head) = argv.first() else {
        return false;
    };
    let head = std::path::Path::new(head)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(head)
        .to_ascii_lowercase();
    match bypass_flag(&head) {
        Some(flag) => argv.iter().any(|a| a == flag),
        None => false,
    }
}

/// Waveform width (number of samples). Advances by one per tick
pub const ACTIVITY_LEN: usize = 24;

/// How long after a submit its own echo/redraw burst is still expected.
/// Screen changes later than this are the peer actually producing output —
/// the secondary "it answered" evidence when the working indicator was missed
const POST_SUBMIT_ECHO_MS: u64 = 2_000;

/// `last_resize_ms` when the tab has never been resized. A real stamp is an
/// age in ms, so it can legitimately be 0 (a resize in the first millisecond);
/// the "not yet" answer needs a value no age can reach.
const NEVER_RESIZED: u64 = u64::MAX;

/// What a launch should do about the conversation.
///
/// Decided by the caller, not here: whether the newest conversation in a folder
/// can safely be continued depends on what the *other* tabs are doing, and only
/// the main loop can see them.
#[derive(Debug, Clone, PartialEq)]
pub enum Resume {
    /// Start a new conversation
    Fresh,
    /// Carry on this exact one
    Id(Session),
    /// Continue whatever ran in this folder last. The caller has checked that
    /// only this tab could have been it
    NewestHere,
}

/// A conversation a tab is running, and how sure we are that it is this tab's.
///
/// The source matters at the moment of resuming: two of these are facts, the
/// third is a match, and resuming the wrong conversation is worse than starting
/// a new one. So a match is used only when nothing else could have produced it.
#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    pub id: String,
    pub source: SessionSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionSource {
    /// We chose the id and launched the CLI with it. Nothing to attribute
    Minted,
    /// The CLI's own hook reported it, holding this tab's API key. Also a fact
    Hook,
    /// We matched the CLI's own records by folder and time. A good guess, and
    /// only ever a guess
    Store,
}

impl Session {
    /// How the id is written down where a person might see it. Never the id
    /// itself: it names a conversation, and a log is not the place for it
    pub fn short(&self) -> String {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in self.id.as_bytes() {
            h ^= *b as u64;
            h = h.wrapping_mul(0x1000_0000_01b3);
        }
        format!("{:?}:{:x}", self.source, h & 0xffff_ffff)
    }
}

pub struct Tab {
    pub title: String,
    /// ID referenced by automation (optional). If unset, the tab name is used to reference it
    pub id: Option<String>,
    /// The conversation this tab's CLI is running, and how that came to be
    /// known. Kept across a restart on purpose: it is the whole point of
    /// knowing it — a restart that starts the conversation over is the damage
    /// this exists to undo
    pub session: Option<Session>,
    /// What this tab's CLI can do about carrying a conversation across a
    /// restart, read from its profile at every launch (so editing a profile
    /// takes effect the next time, like the detection rules do)
    pub resume: Option<crate::profile::ResumeSpec>,
    /// What the thing running here says it is doing, keyed so that several
    /// sources can speak without talking over each other. Newest last
    pub status: Vec<(String, String)>,
    /// Notifications the program asked for through the standard escapes,
    /// waiting for the loop to pick them up
    notes: Notes,
    /// The conversation this tab was having when the app last closed. Not
    /// resumed on its own — that would hand back yesterday's context to
    /// someone who quit to be rid of it — but offered to the key that already
    /// means "carry the conversation over"
    pub previous: Option<Session>,
    /// Whether anything has been said in this tab since it started. Set from
    /// the writing side, which is shared, hence the atomic
    spoke: AtomicBool,
    /// How far along it says it is (0..=1), and what it calls the task
    pub progress: Option<(f32, String)>,
    /// The process we launched. Kept so the ports this tab opened can be
    /// found: what listens is almost never this process, it is whatever it
    /// started, and the only way to ask that question is to start from here
    pub pid: Option<u32>,
    /// Where this tab is -- the branch it sits on, the ports it opened. Filled
    /// in by the loop rather than by the tab, because both answers come from
    /// one look at the whole machine and paying for that per tab would be
    /// paying several times for the same reply
    pub place: crate::repo::Place,
    /// What this tab is costing the machine -- processor and memory across the
    /// agent and everything it started. Also filled in by the loop, from the
    /// same process-tree walk the ports come from
    pub usage: crate::usage::Usage,
    /// The model bridge's endpoint. If Some, this tab is an OpenAI-compatible
    /// API rather than an AI CLI.
    /// When its turn comes, the main process hits complete() on a thread and injects the response into the screen
    pub model: Option<crate::bridge::ModelConn>,
    pub parser: SharedParser,
    pub writer: PtyWriter,
    pub state: TabState,
    pub spinner_idx: usize,
    pub copy: Option<CopyState>,
    /// Depth of the auto-send chain (the "number of times passed along"
    /// recorded on the invisible ball.
    /// Inherited +1 on auto-send, reset to 0 on manual human input)
    pub chain_depth: u32,
    /// Input lock (soft lock). Only guards against human mistakes — auto-send still goes through
    pub locked: bool,
    /// Whether to auto-restart when the child process exits
    pub auto_restart: bool,
    /// Notification destination to ping when this tab finishes a response.
    pub notify_on_done: Option<String>,
    /// Settings changed, but taking effect requires the session to be
    /// recreated
    /// (restart is left to the user so a running AI is never cut off unexpectedly)
    pub needs_restart: bool,
    /// Tab-bar display indent level (0 = top-level)
    pub depth: u16,
    /// Retains the launch conditions for restarting (allows re-spawning with the same settings after the process exits)
    argv: Vec<String>,
    profile_spec: Option<String>,
    opts: TabOptions,
    /// Time of the last manual human input (relative ms). Guards against auto-send immediately after.
    ///
    /// None means "never touched yet." Using 0 to represent that would be
    /// misread as "just touched" for the guard duration right after app startup
    pub last_manual_ms: Option<u64>,
    master: Box<dyn MasterPty + Send>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    child_exited: Arc<AtomicBool>,
    bell_count: Arc<AtomicU64>,
    /// Cumulative bytes read from the PTY (incremented by the reader thread)
    bytes_out: Arc<AtomicU64>,
    /// Time this session was created. Used to judge whether it just started up
    created: Instant,
    /// The same moment on the wall clock, which is what a file's timestamp can
    /// be compared against. A record older than this belongs to an earlier run
    born: std::time::SystemTime,
    /// When to next look for this tab's conversation in its CLI's own records,
    /// and how many looks are left. `None` once there is nothing to look for —
    /// the conversation is known, the CLI keeps no records, or it never
    /// started one
    pub session_probe: Option<(Instant, u8)>,
    /// Time of the most recent resize (elapsed ms since creation).
    ///
    /// A child process redraws the screen when the terminal size changes.
    /// The content is unchanged but the screen still moves, so counting that
    /// directly as activity would walk through BUSY→DONE and make it look
    /// like a response had arrived
    last_resize_ms: AtomicU64,
    /// Whether the "working" indicator was seen since execution.
    ///
    /// Whether the screen changed isn't enough by itself. Even a paste
    /// redrawing into the `[Pasted Content …]` form changes the screen, but
    /// the AI hasn't done anything
    saw_working: AtomicBool,
    /// When (on the tick clock) the current submit was first observed.
    /// `u64::MAX` = no submit pending. Latched by the first tick after a
    /// submit, so "how long after the submit did output still move" can be
    /// answered on the same clock `last_change_ms` uses
    submit_tick_ms: AtomicU64,
    /// Hash of the screen content at the moment of execution.
    ///
    /// Using output byte count instead would count cursor blinking or frame
    /// redraws too, misreading them as "answered." If execution never
    /// landed, the screen content doesn't change
    submitted_screen: AtomicU64,
    /// Cumulative output amount at the moment of execution.
    ///
    /// If execution never reached the other side, the screen goes quiet
    /// while still showing the paste. That "moved → stopped" shape looks
    /// just like a response, so without checking whether output occurred
    /// after execution, something never answered would be treated as answered
    submitted_output: AtomicU64,
    /// Whether we're waiting for a response to a submitted input.
    ///
    /// What matters is "submitted," not merely "typed." Just typing or just
    /// pasting also makes the screen move and then stop, so using input
    /// alone as the basis would misread the moment typing paused as a
    /// completed response
    prompted: AtomicBool,
    /// History of recent output volume (oldest → newest). For the INDEX waveform
    activity: [u8; ACTIVITY_LEN],
    /// Cumulative byte count at the time of the previous sample
    activity_mark: u64,
    last_hash: u64,
    last_change_ms: u64,
    /// Capture of the latest response (DESIGN 7.3: submit-boundary marker scheme)
    pub last_response: Option<String>,
    /// Start position of the response (scrollback accumulation amount). u64::MAX = unset.
    ///
    /// What matters is "the position where execution happened," not "the
    /// first position where the screen moved." A paste being displayed or
    /// the input box redrawing also moves the screen, so taking it from the
    /// moved position would grab the frame instead of the answer
    response_marker: AtomicU64,
    /// Whether the screen width narrowed while waiting for a response.
    ///
    /// Row numbers don't move when the size changes, so the extraction range
    /// is preserved. But narrowing the width makes vt100 truncate each row
    /// to that width, chopping off the text itself. This can't be undone
    /// from our side, so at least make it possible to know it may have been chopped
    resized_while_waiting: AtomicBool,
    /// The visible screen at the moment of execution (each row, top to bottom).
    ///
    /// A full-screen TUI (like Claude Code) doesn't scroll, so row numbers
    /// don't advance and "rows written since execution" can't be counted.
    /// Instead, compare against the pre-execution screen and strip out
    /// whatever was already in the same place back then (startup banner, frame)
    submitted_rows: Mutex<Vec<String>>,
    detector: Detector,
    /// Direct-chat conversation with a model tab (true = a human turn, false =
    /// the model's reply). The bridge is stateless, so on each send the whole
    /// history is replayed. Behind an Arc<Mutex> so the reply thread can append.
    /// Empty and unused for non-model (CLI) tabs.
    chat_history: Arc<Mutex<Vec<(bool, String)>>>,
    /// Whether a chat reply is currently being generated. Drives the spinner in
    /// the UI and is set/cleared by `chat_send`'s thread.
    model_busy: Arc<AtomicBool>,
    /// A browser-brain model's latest reply, verbatim. The reply thread stores
    /// it here so the rally orchestrator's `on_done` can pull the ```lua block
    /// out of the exact text (the on-screen copy is line-wrapped to the tab
    /// width, which would split long URLs). None until the first reply / for
    /// non-brain tabs.
    last_model_reply: Arc<Mutex<Option<String>>>,
}

impl Tab {
    /// The tab's working folder (where its process was launched). Attachments are
    /// written here — under `.SHIKISHA/tmp/` — so the AI running in this folder
    /// can reach them by the path we hand back.
    pub fn cwd(&self) -> Option<&std::path::Path> {
        self.opts.cwd.as_deref()
    }

    /// The profile is resolved fresh each time, either from a name (given in
    /// config) or from the command name.
    /// Because it's re-resolved on restart, edits to profiles/*.json take effect immediately
    pub(crate) fn resolve_profile(argv: &[String], spec: &Option<String>) -> Profile {
        match spec {
            Some(name) => crate::profile::load_by_name(name),
            None => crate::profile::load_for_command(argv.first().map(String::as_str).unwrap_or("")),
        }
    }

    /// Launch a tab with a new conversation.
    pub fn spawn(
        title: String,
        argv: &[String],
        profile_spec: Option<String>,
        rows: u16,
        cols: u16,
        opts: TabOptions,
    ) -> Result<Self> {
        Self::spawn_as(title, argv, profile_spec, rows, cols, opts, Resume::Fresh)
    }

    /// Launch a tab, saying what should happen to the conversation.
    #[allow(clippy::too_many_arguments)]
    pub fn spawn_as(
        title: String,
        argv: &[String],
        profile_spec: Option<String>,
        rows: u16,
        cols: u16,
        opts: TabOptions,
        plan: Resume,
    ) -> Result<Self> {
        let profile = Self::resolve_profile(argv, &profile_spec);
        let pty_system = native_pty_system();
        let pair = pty_system.openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        // A model tab doesn't start a real CLI — it starts an idle process
        // that just holds the display.
        // The turn's response is injected into the parser by the main
        // process hitting complete() on a thread
        // (the main process is a GUI subsystem, and making the bridge a ConPTY child would leave it without I/O)
        let idle;
        // What the conversation asks for, turned into arguments. A model tab
        // runs no CLI at all, so there is nothing to resume
        let resume_spec = if opts.model.is_some() { None } else { profile.resume.clone() };
        let (resumed, session) = plan_launch(resume_spec.as_ref(), argv, plan);
        let spawn_argv: &[String] = if opts.model.is_some() {
            idle = idle_argv();
            &idle
        } else {
            &resumed
        };
        let mut cmd = build_command(spawn_argv);
        // Where the external API is, the key to it, and which tab this is.
        // Done here because this is the one place a tab's process is born —
        // a CLI started anywhere else would silently have no way to call home
        for (k, v) in crate::api::child_env(&title) {
            cmd.env(k, v);
        }
        // If unspecified/nonexistent, launch in the app's folder
        // (passing a nonexistent folder would make the launch itself fail)
        let cwd = opts
            .cwd
            .clone()
            .filter(|p| p.is_dir())
            .unwrap_or(std::env::current_dir()?);
        cmd.cwd(cwd);
        let mut child = pair.slave.spawn_command(cmd)?;
        drop(pair.slave);
        let pid = child.process_id();
        let killer = child.clone_killer();

        let writer: PtyWriter = Arc::new(Mutex::new(pair.master.take_writer()?));
        let bell_count = Arc::new(AtomicU64::new(0));
        // Cumulative output volume. The INDEX waveform is drawn from its deltas
        // (the change in screen hash alone doesn't tell us "how much is moving")
        let bytes_out = Arc::new(AtomicU64::new(0));
        let notes: Notes = Arc::new(Mutex::new(Vec::new()));
        let parser: SharedParser = Arc::new(Mutex::new(vt100::Parser::new_with_callbacks(
            rows,
            cols,
            opts.scrollback,
            QueryResponder {
                writer: Arc::clone(&writer),
                bell: Arc::clone(&bell_count),
                notes: Arc::clone(&notes),
            },
        )));
        // A model-bridge tab launches an idle placeholder process, so its screen
        // would otherwise be blank. Paint a small title card (like the CLIs show
        // on startup) so it reads as a real, identified endpoint. Done straight
        // on the parser, not counted as output, so it doesn't look like activity.
        if let Some(conn) = opts.model.as_ref() {
            if let Ok(mut p) = parser.lock() {
                p.process(model_title_box(conn, cols).as_bytes());
            }
        }
        let child_exited = Arc::new(AtomicBool::new(false));

        // PTY output → (encoding conversion if needed) → vt100 parser / session log
        {
            let parser = Arc::clone(&parser);
            let counter = Arc::clone(&bytes_out);
            let mut reader = pair.master.try_clone_reader()?;
            let enc = opts.encoding;
            let mut log = opts
                .log
                .then(|| crate::session_log::SessionLog::open(&crate::config::logs_dir(), &title));
            std::thread::spawn(move || {
                let mut buf = [0u8; 8192];
                let mut decoder = enc.map(|e| e.new_decoder());
                let mut text = String::new();
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            counter.fetch_add(n as u64, Ordering::Relaxed);
                            let chunk: &[u8] = match decoder.as_mut() {
                                // Convert Shift_JIS etc to UTF-8 before passing to the parser
                                Some(d) => {
                                    text.clear();
                                    text.reserve(n * 3);
                                    let _ = d.decode_to_string(&buf[..n], &mut text, false);
                                    text.as_bytes()
                                }
                                None => &buf[..n],
                            };
                            if let Some(l) = log.as_mut() {
                                l.write(chunk);
                            }
                            // vt100 can panic on how it handles full-width
                            // characters after narrowing the width
                            // (observed: unwrap fires when the right edge
                            // stays full-width after narrowing and a
                            // half-width character is written).
                            // If it panics, rebuild the parser and keep reading.
                            // Giving up here would mean the tab shows nothing ever again
                            let hit = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                                || {
                                    parser
                                        .lock()
                                        .unwrap_or_else(|e| e.into_inner())
                                        .process(chunk);
                                },
                            ));
                            if hit.is_err() {
                                // Continuing to read in the broken state would
                                // panic again on the very next character.
                                // Send a full terminal reset to return to a known state
                                let reset = std::panic::catch_unwind(
                                    std::panic::AssertUnwindSafe(|| {
                                        parser
                                            .lock()
                                            .unwrap_or_else(|e| e.into_inner())
                                            .process(b"\x1bc");
                                    }),
                                );
                                crate::append_hook_log(if reset.is_ok() {
                                    "Screen parsing broke, so it was rebuilt (width change and full-width characters)"
                                } else {
                                    "Screen parsing broke, and rebuilding it also failed"
                                });
                            }
                        }
                    }
                }
            });
        }
        // Detect child process exit
        {
            let flag = Arc::clone(&child_exited);
            std::thread::spawn(move || {
                let _ = child.wait();
                flag.store(true, Ordering::SeqCst);
            });
        }

        Ok(Self {
            notes,
            previous: None,
            spoke: AtomicBool::new(false),
            status: Vec::new(),
            progress: None,
            pid,
            place: crate::repo::Place::default(),
            usage: crate::usage::Usage::default(),
            born: std::time::SystemTime::now(),
            // Look for it shortly: a CLI writes its record as the conversation
            // begins, which is a moment after the process starts
            session_probe: (session.is_none()
                && resume_spec.as_ref().is_some_and(|r| r.record.is_some()))
            .then(|| (Instant::now() + std::time::Duration::from_secs(1), 30)),
            session,
            resume: resume_spec,
            title,
            id: None,
            model: opts.model.clone(),
            parser,
            writer,
            state: TabState::Wait,
            spinner_idx: 0,
            copy: None,
            chain_depth: 0,
            locked: false,
            auto_restart: false,
            notify_on_done: None,
            needs_restart: false,
            depth: 0,
            argv: argv.to_vec(),
            profile_spec,
            opts,
            last_manual_ms: None,
            master: pair.master,
            killer,
            child_exited,
            bell_count,
            bytes_out,
            created: Instant::now(),
            prompted: AtomicBool::new(false),
            submitted_output: AtomicU64::new(0),
            submitted_screen: AtomicU64::new(0),
            saw_working: AtomicBool::new(false),
            submit_tick_ms: AtomicU64::new(u64::MAX),
            last_resize_ms: AtomicU64::new(NEVER_RESIZED),
            activity: [0; ACTIVITY_LEN],
            activity_mark: 0,
            last_hash: 0,
            last_change_ms: 0,
            last_response: None,
            response_marker: AtomicU64::new(u64::MAX),
            resized_while_waiting: AtomicBool::new(false),
            submitted_rows: Mutex::new(Vec::new()),
            detector: Detector::new(profile),
            chat_history: Arc::new(Mutex::new(Vec::new())),
            model_busy: Arc::new(AtomicBool::new(false)),
            last_model_reply: Arc::new(Mutex::new(None)),
        })
    }

    /// Whether we're waiting for a response to a submitted input
    pub fn was_prompted(&self) -> bool {
        self.prompted.load(Ordering::Relaxed)
    }

    /// Current screen content (excluding the bottom decoration)
    fn screen_fingerprint(&self) -> u64 {
        let p = self.parser.lock().unwrap_or_else(|e| e.into_inner());
        screen_hash(p.screen(), self.detector.ignore_bottom_rows())
    }

    /// Raw value of "was the started-working indicator seen" (for logging)
    pub fn saw_working_flag(&self) -> bool {
        self.saw_working.load(Ordering::Relaxed)
    }

    /// Whether execution reached the other side and a response actually started.
    ///
    /// While working, an AI CLI shows it on screen (e.g. "esc to interrupt").
    /// If execution never landed, the paste just sits in the input box and
    /// that indicator never appears.
    /// Whether the screen changed isn't enough on its own —
    /// even a paste redrawing into `[Pasted Content …]` changes the screen.
    ///
    /// For a peer without a "working" indicator (a plain shell, no profile
    /// configured), fall back to the screen change after the post-execution
    /// redraw settles.
    ///
    /// This is set up so the strength of the guard doesn't depend on whether
    /// a profile exists. Previously, having no profile weakened the whole
    /// judgment, and a mere paste redraw alone would count as "answered"
    pub fn answered_since_submit(&self) -> bool {
        if self.detector.shows_working() {
            // For a peer that shows a working indicator, its appearance is
            // the primary evidence. More reliable than whether the screen
            // moved, and not fooled by redraws
            if self.saw_working.load(Ordering::Relaxed) {
                return true;
            }
            // Secondary evidence: output kept moving well after the submit.
            // The indicator is a UI string that CLIs restyle between
            // versions, and a short turn can slip between polls; treating
            // its absence as final turns a real answer into a permanent
            // "no response" and deadlocks whoever is waiting on it. The
            // echo of the submitted prompt can't fake this signal — it's a
            // one-shot burst right at the submit, while an actual answer
            // streams in afterwards
            let submit_ms = self.submit_tick_ms.load(Ordering::Relaxed);
            return submit_ms != u64::MAX
                && self.last_change_ms > submit_ms.saturating_add(POST_SUBMIT_ECHO_MS)
                && self.screen_fingerprint() != self.submitted_screen.load(Ordering::Relaxed);
        }
        // For a peer that doesn't show one (plain shell, no profile
        // configured), fall back to the screen change. The baseline is the
        // screen at the moment execution was sent, and since execution is
        // sent only after the paste finishes being taken in, the screen at
        // that point has already settled.
        // If it never landed, nothing moves; if it landed, the answer appears
        self.screen_fingerprint() != self.submitted_screen.load(Ordering::Relaxed)
    }

    /// The response has been fully received, so go back to waiting for the next execution
    pub fn finish_response(&mut self) {
        self.prompted.store(false, Ordering::Relaxed);
        self.response_marker.store(u64::MAX, Ordering::Relaxed);
    }

    /// Send input. Only input containing a submit counts as "requesting a response"
    /// Input that just flows straight through to the child process.
    /// Not input that requests a response, so `prompted` is not set
    /// (used only for real-machine verification. The normal path is write_bytes)
    #[cfg(test)]
    pub fn write_passthrough(&self, bytes: &[u8]) -> Result<()> {
        pty_write(&self.writer, bytes)
    }

    pub fn write_bytes(&self, bytes: &[u8]) -> Result<()> {
        if contains_submit(bytes) {
            self.prompted.store(true, Ordering::Relaxed);
            // Something has now been said in this tab, in this run. That is
            // what decides which conversation "carry it over" means: the one
            // happening here, or — on a tab nobody has spoken to yet — the one
            // that was happening when the app last closed.
            //
            // Said, not busy: a CLI looks busy while it merely starts up, and
            // treating that as use would quietly discard the conversation the
            // person came back for
            self.spoke.store(true, Ordering::Relaxed);
            self.submitted_output
                .store(self.output_count(), Ordering::Relaxed);
            // The response starts here. Anything before this is the
            // instruction, not the answer.
            //
            // The +1 is because execution is precisely the action that
            // finishes writing the current row. Using the cursor row itself
            // as the starting point would include the last line of the
            // typed instruction (or just its back half, if wrapped) in the answer
            self.response_marker
                .store(self.line_position() as u64 + 1, Ordering::Relaxed);
            self.submitted_screen
                .store(self.screen_fingerprint(), Ordering::Relaxed);
            self.saw_working.store(false, Ordering::Relaxed);
            self.submit_tick_ms.store(u64::MAX, Ordering::Relaxed);
            self.resized_while_waiting.store(false, Ordering::Relaxed);
            *self.submitted_rows.lock().unwrap() = self.visible_rows();
        }
        // If the peer isn't UTF-8, convert the characters we send too
        // (control sequences are ASCII, so they pass through unchanged)
        if let Some(enc) = self.opts.encoding {
            if let Ok(s) = std::str::from_utf8(bytes) {
                let (encoded, _, _) = enc.encode(s);
                return pty_write(&self.writer, &encoded);
            }
        }
        pty_write(&self.writer, bytes)
    }

    /// Whether we're within the redraw window.
    ///
    /// What we wait for is the redraw itself finishing, not the screen
    /// settling down. Waiting too long would miss the start of a real response.
    ///
    /// A tab that has never been resized is NOT redrawing, however young it is.
    /// Marking "no resize yet" with 0 made every tab spend its first 800ms
    /// claiming to redraw, so the startup banner never counted as activity, the
    /// detector never left WAIT, and `ready_for_startup_hook` waved the
    /// automation through into a CLI that had not drawn its input box yet --
    /// the persona landed in the prompt as an unsent draft.
    fn redrawing(&self) -> bool {
        const REDRAW_MS: u64 = 800;
        let at = self.last_resize_ms.load(Ordering::Relaxed);
        at != NEVER_RESIZED && self.age_ms().saturating_sub(at) < REDRAW_MS
    }

    /// Whether the peer has declared that it understands bracketed paste.
    ///
    /// By terminal convention, a supporting app declares this itself by
    /// sending ESC[?2004h. If sent with markers to a peer that hasn't
    /// declared it (a plain shell), the markers are ignored and the
    /// newlines inside become real submits. We judge by this declaration, not by guessing
    pub fn accepts_bracketed_paste(&self) -> bool {
        self.parser.lock().unwrap_or_else(|e| e.into_inner()).screen().bracketed_paste()
    }

    /// Whether the width narrowed while waiting for a response (the text may have been chopped)
    pub fn resized_while_waiting(&self) -> bool {
        self.resized_while_waiting.load(Ordering::Relaxed)
    }

    pub fn resize(&self, rows: u16, cols: u16) -> Result<()> {
        let narrower = {
            let p = self.parser.lock().unwrap_or_else(|e| e.into_inner());
            let (_, old_cols) = p.screen().size();
            cols < old_cols
        };
        // Content is only lost when the width narrows. Height affects neither row numbers nor content
        if narrower && self.prompted.load(Ordering::Relaxed) {
            self.resized_while_waiting.store(true, Ordering::Relaxed);
        }
        self.last_resize_ms.store(self.age_ms(), Ordering::Relaxed);
        self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        self.parser.lock().unwrap_or_else(|e| e.into_inner()).screen_mut().set_size(rows, cols);
        Ok(())
    }

    pub fn exited(&self) -> bool {
        self.child_exited.load(Ordering::SeqCst)
    }

    pub fn kill(&mut self) {
        let _ = self.killer.kill();
    }

    /// Recreate the session with the same settings.
    /// Used to recover from a child process self-update, SSH disconnect, or crash.
    /// Lock state and hierarchy display carry over; chain depth and history are reset
    /// Relaunch, carrying the conversation over. What that means for this CLI
    /// is decided by the caller — see `Resume`
    pub fn restart_as(&mut self, rows: u16, cols: u16, plan: Resume) -> Result<()> {
        self.kill();
        let mut fresh = Tab::spawn_as(
            self.title.clone(),
            &self.argv.clone(),
            self.profile_spec.clone(),
            rows,
            cols,
            self.opts.clone(),
            plan,
        )?;
        fresh.locked = self.locked;
        fresh.depth = self.depth;
        fresh.auto_restart = self.auto_restart;
        fresh.id = self.id.clone();
        fresh.notify_on_done = self.notify_on_done.clone();
        fresh.previous = self.previous.clone();
        fresh.clear_said();
        // Since it was recreated, any pending config changes are now in effect
        *self = fresh;
        Ok(())
    }

    /// Re-resolve a model tab's connection from the current provider table,
    /// keeping the persona and browser-drive role it was launched with.
    ///
    /// Model tabs are resolved at spawn, which happens before the master
    /// password is entered. With an encrypted secrets file the api_key can't be
    /// read yet, so the tab starts with an empty key (→ HTTP 401). Once the
    /// password unlocks the providers, call this to pick up the real key.
    pub fn refresh_model_conn(&mut self) {
        let Some(old) = self.model.as_ref() else {
            return;
        };
        if let Some(mut fresh) = crate::bridge::launch_for(&self.argv) {
            fresh.persona = old.persona.clone();
            fresh.drives = old.drives.clone();
            self.model = Some(fresh);
        }
    }

    /// Explains, in plain language, why this tab failed to (re)start — a missing
    /// program or a missing working folder, the two things a portable build runs
    /// into when it lands on a PC that isn't the one it was configured on.
    pub fn launch_hint(&self, raw: &str) -> String {
        launch_problem(
            &self.title,
            self.argv.first().map(String::as_str).unwrap_or(""),
            self.opts.cwd.as_deref(),
            raw,
        )
    }

    pub fn profile_name(&self) -> &str {
        self.detector.profile_name()
    }

    /// Which AI this tab runs, as a lowercase identity the UI can brand: a
    /// model bridge's provider (deepseek / qwen / …), or a CLI's command head
    /// (claude / codex / gemini / aider / kimi). None for shells and anything
    /// unrecognized. This is a fact about the tab, not a look — the display
    /// side maps it to a colour.
    /// The command line this tab was started with ("ssh user@host", "wsl", …).
    /// Context for the ✨ command suggester: what the terminal connects to
    pub fn command_line(&self) -> String {
        self.argv.join(" ")
    }

    /// The one line a tab row has room for: the most recent thing said.
    ///
    /// Not a merge of every key — a row eighteen columns wide cannot show
    /// three things, and picking the newest is a rule anyone can predict. The
    /// whole set is on the board, which has the room for it
    pub fn status_line(&self) -> Option<String> {
        let last = self.status.last().map(|(_, v)| v.clone())?;
        Some(match self.progress {
            Some((p, _)) => format!("{last} {}%", (p * 100.0).round() as i32),
            None => last,
        })
    }

    /// Say what is happening now. An empty value takes that entry away, so
    /// finishing needs no second verb
    pub fn set_status(&mut self, key: &str, value: &str) {
        self.status.retain(|(k, _)| k != key);
        if !value.trim().is_empty() {
            self.status.push((key.to_string(), value.trim().to_string()));
        }
    }

    /// Lines in this tab's scrollback that contain `needle`, newest first.
    ///
    /// For searching every open tab at once: the answer to "which of my agents
    /// mentioned that error". Bounded on both ends -- only the most recent
    /// stretch of history, and only so many hits -- because this runs on every
    /// keystroke across every tab, and a person is looking for something they
    /// saw recently, not on the thousandth line back
    pub fn search_lines(&self, needle: &str, max_hits: usize) -> Vec<(usize, String)> {
        let needle = needle.trim().to_lowercase();
        if needle.is_empty() {
            return Vec::new();
        }
        let mut p = self.parser.lock().unwrap_or_else(|e| e.into_inner());
        let (rows, cols) = p.screen().size();
        let back = furthest_back(&mut p);
        let top = back + (rows as usize).saturating_sub(1);
        // Only the recent stretch -- far enough to cover what is on screen and
        // a good deal above it, not the whole kept history
        let scan = top.min(3000);
        let mut out = Vec::new();
        for d in 0..=scan {
            if out.len() >= max_hits {
                break;
            }
            let line = line_at(&mut p, d, cols);
            if line.to_lowercase().contains(&needle) {
                let text = line.trim().to_string();
                if !text.is_empty() {
                    out.push((d, text));
                }
            }
        }
        out
    }

    /// Take whatever the program asked us to notice since last time.
    ///
    /// This is the one way in that needs nothing set up: a CLI that has never
    /// heard of this app, run over ssh or inside a container, still knows how
    /// to ring a terminal
    pub fn take_notes(&self) -> Vec<(String, String)> {
        self.notes
            .lock()
            .map(|mut n| std::mem::take(&mut *n))
            .unwrap_or_default()
    }

    /// Everything said, oldest first, for the board
    pub fn status_all(&self) -> &[(String, String)] {
        &self.status
    }

    /// A restart is the same tab doing the same job; what it said it was doing
    /// belongs to the process that just died
    fn clear_said(&mut self) {
        self.status.clear();
        self.progress = None;
    }

    /// Whether anything has been said in this tab since it started
    pub fn spoke(&self) -> bool {
        self.spoke.load(Ordering::Relaxed)
    }

    /// When this tab's process started, on the wall clock
    pub fn born(&self) -> std::time::SystemTime {
        self.born
    }

    /// The program this tab runs, without its arguments. Two tabs sharing this
    /// AND a working folder are writing into the same CLI's records, which is
    /// what makes "the newest conversation here" ambiguous
    pub fn program(&self) -> &str {
        self.argv.first().map(String::as_str).unwrap_or_default()
    }

    pub fn ai_kind(&self) -> Option<String> {
        if let Some(conn) = self.model.as_ref() {
            let p = conn.provider.trim().to_ascii_lowercase();
            return (!p.is_empty()).then_some(p);
        }
        let head = self.argv.first()?;
        let head = std::path::Path::new(head)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(head)
            .to_ascii_lowercase();
        matches!(head.as_str(), "claude" | "codex" | "gemini" | "aider" | "kimi")
            .then_some(head)
    }

    /// Whether this tab runs without pausing for per-action confirmation — the
    /// prerequisite for driving another tab (operate), autonomous discussion, and
    /// automation. A model bridge always does (it writes replies in-process); a
    /// CLI does only when launched with its "act without asking" flag. Anything
    /// else (aider / kimi / a plain shell) has no such flag, so it never does.
    pub fn auto_runs(&self) -> bool {
        argv_auto_runs(&self.argv, self.model.is_some())
    }

    /// Fingerprint of the launch conditions. If this changes, the session needs to be recreated
    pub fn signature(&self) -> String {
        signature_of(&self.argv, &self.opts)
    }

    /// How automation identifies this tab
    pub fn key(&self) -> crate::hooks::TabKey {
        crate::hooks::TabKey {
            id: self.id.clone(),
            name: self.title.clone(),
        }
    }

    /// Swap in settings that can take effect without a restart
    pub fn apply_live_config(&mut self, profile_spec: Option<String>, locked: bool, auto_restart: bool, depth: u16, notify_on_done: Option<String>) {
        if self.profile_spec != profile_spec {
            self.profile_spec = profile_spec;
            self.detector = Detector::new(Self::resolve_profile(&self.argv, &self.profile_spec));
        }
        self.locked = locked;
        self.auto_restart = auto_restart;
        self.depth = depth;
        self.notify_on_done = notify_on_done;
    }

    /// Stash the launch conditions to use on the next restart (keeps running with the current settings until then)
    pub fn stage_restart_config(&mut self, argv: Vec<String>, opts: TabOptions) {
        self.argv = argv;
        self.opts = opts;
        self.needs_restart = true;
    }

    /// State judgment every 200ms (call this for inactive tabs too).
    /// Activity is judged by "screen content change" (excluding the bottom status row).
    /// Returns (old state, new state) for firing hooks
    pub fn tick(&mut self, start: Instant) -> (TabState, TabState) {
        if self.exited() {
            let old = self.state;
            self.state = TabState::Exited;
            return (old, self.state);
        }
        let (screen_text, hash) = {
            let p = self.parser.lock().unwrap_or_else(|e| e.into_inner());
            let screen = p.screen();
            (
                screen.contents(),
                screen_hash(screen, self.detector.ignore_bottom_rows()),
            )
        };
        let now = start.elapsed().as_millis() as u64;
        if hash != self.last_hash {
            self.last_hash = hash;
            // A post-resize redraw isn't new output, so it doesn't count as activity
            if !self.redrawing() {
                self.last_change_ms = now;
            }
        }
        let since = now.saturating_sub(self.last_change_ms);
        // Stamp the pending submit with this clock (write_bytes can't — it
        // doesn't see the tick epoch), so "did output move well after the
        // submit" is answerable in answered_since_submit
        if self.prompted.load(Ordering::Relaxed)
            && self.submit_tick_ms.load(Ordering::Relaxed) == u64::MAX
        {
            self.submit_tick_ms.store(now, Ordering::Relaxed);
        }
        let old_state = self.state;
        self.state = self
            .detector
            .tick(&screen_text, since, self.bell_count.load(Ordering::Relaxed));
        // A model bridge is working with nothing on screen to show for it: the
        // request is in flight over HTTP and not a pixel moves until the reply
        // lands. The detector only ever watches the screen, so it read that
        // silence as DONE, on_done fired on a turn that had not happened, and
        // an orchestrator waiting for a statement found none -- and asked
        // again, and again. The pane itself knows whether it is waiting; the
        // detector is told rather than left to guess.
        if self.is_generating() {
            self.state = TabState::Busy;
        }
        if self.state == TabState::Busy {
            self.spinner_idx = self.spinner_idx.wrapping_add(1);
        }
        if self.detector.working_shown() {
            if !self.saw_working.swap(true, Ordering::Relaxed) {
                // Record what evidence led us to see "started working."
                // If it picked up screen decoration by mistake, it shows up here
                crate::append_hook_log(&format!(
                    "working tab? [{}] match: {:?}",
                    self.detector.profile_name(),
                    self.detector.working_matched()
                ));
            }
        }
        self.sample_activity();

        // Response capture (submit-boundary marker scheme):
        // record the scrollback accumulation amount at the moment BUSY
        // starts as the boundary, and on DONE extract only what's after
        // that boundary (past responses never get mixed in)
        if old_state == TabState::Busy && self.state == TabState::Done {
            self.last_response = Some(self.capture_since_marker());
        }
        (old_state, self.state)
    }

    /// Push this tick's output volume onto the history.
    /// Raw byte counts swing too wildly in magnitude, so collapse them
    /// logarithmically into 0..=7 levels
    /// (enough to distinguish "quiet / trickling / flowing")
    fn sample_activity(&mut self) {
        let total = self.bytes_out.load(Ordering::Relaxed);
        let delta = total.saturating_sub(self.activity_mark);
        self.activity_mark = total;
        let level = match delta {
            0 => 0,
            1..=31 => 1,
            32..=127 => 2,
            128..=511 => 3,
            512..=2047 => 4,
            2048..=8191 => 5,
            8192..=32767 => 6,
            _ => 7,
        };
        self.activity.rotate_left(1);
        self.activity[ACTIVITY_LEN - 1] = level;
    }

    /// This AI's specific confirmation time (if given in the profile)
    pub fn done_confirm_ms(&self) -> Option<u64> {
        self.detector.done_confirm_ms()
    }

    /// Recent output volume (oldest → newest, each 0..=7)
    pub fn activity(&self) -> &[u8] {
        &self.activity
    }

    /// How long the meaningful screen contents have been unchanged, in ms
    /// (post-resize redraws excluded, same as the activity timer). Unlike the
    /// BUSY verdict this reads the raw screen, so a program that parks a static
    /// status footer on screen (e.g. Claude Code's "✳ Crunched for 15s") can't
    /// masquerade as still working. `now_ms` must come from the same clock as
    /// `Tab::tick` (the launch `Instant`'s elapsed millis).
    pub fn ms_since_change(&self, now_ms: u64) -> u64 {
        now_ms.saturating_sub(self.last_change_ms)
    }

    /// Whether the child process has output anything (a proxy for whether it started up and is moving)
    pub fn had_output(&self) -> bool {
        self.output_count() > 0
    }

    /// Whether this tab is a model bridge (API)
    pub fn is_model(&self) -> bool {
        self.model.is_some()
    }

    /// Whether a chat reply is being generated right now (for the UI spinner).
    pub fn is_generating(&self) -> bool {
        self.model_busy.load(Ordering::Relaxed)
    }

    /// Aim (or unaim) a model tab at a browser.
    ///
    /// A model tab is a browser brain only while it is aimed at one, and this
    /// is what says so. It decides the system prompt the model is given and
    /// whether its turn is marked for the orchestrator to pick up. That used to
    /// be read from the settings file at launch, so a tab aimed on screen went
    /// on answering as a plain chat — its fenced Lua arriving mangled by the
    /// terminal's line wrapping, because nothing had told it it was driving.
    pub fn set_brain(&mut self, browser: Option<String>) {
        if let Some(c) = self.model.as_mut() {
            c.drives = browser.filter(|b| !b.trim().is_empty());
        }
    }

    /// True when this model tab is a browser-operation *brain* (it is aimed at
    /// a browser): it steers that page by emitting Lua in its reply, so its
    /// turns must fire `on_done` and its reply is kept verbatim for the
    /// orchestrator.
    pub fn is_browser_brain(&self) -> bool {
        self.model
            .as_ref()
            .and_then(|c| c.drives.as_deref())
            .is_some_and(|d| !d.trim().is_empty())
    }

    /// The brain's latest reply, verbatim (None for CLI / plain-chat tabs, or
    /// before the first reply). Cloned so the on_done handler reads the exact
    /// text rather than the line-wrapped screen copy.
    pub fn model_reply(&self) -> Option<String> {
        self.last_model_reply
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Write straight onto this tab's screen, as if the peer had printed it.
    ///
    /// The one way anything reaches a screen without going through the peer:
    /// the model bridge's own turn markers and replies, and `shikisha.note`.
    /// The byte counter is bumped along with the parser so detection sees the
    /// same activity it would see from real output -- a screen that changed
    /// while the activity meter stayed flat reads as a ghost.
    fn inject_into(parser: &SharedParser, counter: &Arc<AtomicU64>, s: &str) {
        let bytes = s.as_bytes();
        counter.fetch_add(bytes.len() as u64, Ordering::Relaxed);
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            parser
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .process(bytes);
        }));
    }

    /// Put a note on this tab's screen. Nothing is sent anywhere and nobody is
    /// asked to answer -- it is a message for the person watching.
    ///
    /// A model bridge is stateless, so it cannot be briefed at startup the way
    /// a CLI can: prompt it and it answers immediately, before any topic
    /// exists. Left at that it sat there blank, and a discussion's participants
    /// were invisible until their turn came round. Telling the screen is not
    /// the same act as telling the peer, so it is its own primitive.
    pub fn note(&self, text: &str) {
        let body = text.trim_end().replace('\n', "\r\n");
        Self::inject_into(
            &self.parser,
            &self.bytes_out,
            &format!("\r\n\x1b[2m{body}\x1b[0m\r\n"),
        );
    }

    /// A human line typed into the chat box. Echoes the line with a
    /// Claude-style prompt marker, then the model replies.
    pub fn chat_send(&self, user_text: String) {
        self.model_turn(user_text, true);
    }

    /// A relayed rally turn (the orchestrator handing back the browser screen).
    /// Same conversation, but the (potentially huge) context isn't echoed as a
    /// prompt line — only a compact marker is shown, then the model's reply.
    pub fn rally_relay(&self, context: String) {
        self.model_turn(context, false);
    }

    /// Shared core of a model turn. `echo` controls whether the incoming text
    /// is shown verbatim (a human's line) or as a compact marker (relayed rally
    /// context). Either way the turn is marked, so BUSY→DONE→on_done fires and
    /// the reply is stashed verbatim for whatever is orchestrating this pane.
    fn model_turn(&self, incoming: String, echo: bool) {
        let Some(conn) = self.model.clone() else { return };
        let text = incoming.trim().to_string();
        if text.is_empty() {
            return;
        }
        let brain = self.is_browser_brain();
        let parser = Arc::clone(&self.parser);
        let counter = Arc::clone(&self.bytes_out);
        let busy = Arc::clone(&self.model_busy);
        let history = Arc::clone(&self.chat_history);
        let last_reply = Arc::clone(&self.last_model_reply);
        busy.store(true, Ordering::Relaxed);
        // Start the turn with no stashed reply, so if this turn errors out the
        // orchestrator won't re-extract and re-run the *previous* turn's ```lua.
        *last_reply.lock().unwrap_or_else(|e| e.into_inner()) = None;
        history
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((true, text.clone()));
        // Every model turn is marked, so detection sees BUSY→DONE and fires
        // on_done. Marking here mirrors dispatch_model.
        //
        // It used to be done for a browser brain only, on the reasoning that a
        // line typed into a chat pane has no orchestrator waiting on it. It
        // can: a discussion's opening speaker is handed the topic by a person,
        // and if that pane is a model bridge its answer was the one turn in the
        // whole round that detection ignored -- so the discussion never started.
        // With no hook attached, firing on_done reaches nobody and costs
        // nothing, which is the right price for not having to know in advance
        // who is listening.
        self.mark_turn_start();
        std::thread::spawn(move || {
            let inject = |s: &str| Self::inject_into(&parser, &counter, s);
            if echo {
                // The human's line, with a Claude-style prompt marker. The
                // "generating" state is shown by the HTML thinking bubble
                // (driven by model_busy), not a text line.
                inject(&format!(
                    "\r\n\x1b[1;32m❯\x1b[0m {}\r\n",
                    text.replace('\n', "\r\n")
                ));
            } else {
                // Relayed context: a compact dim marker instead of the dump.
                inject(&format!(
                    "\r\n\x1b[2m… {}\x1b[0m\r\n",
                    crate::i18n::t("agent.browser.model.relayed")
                ));
            }
            // Replay the whole history (the bridge keeps no state of its own).
            let msgs = {
                let h = history.lock().unwrap_or_else(|e| e.into_inner());
                let mut msgs = Vec::new();
                // A brain gets the browser-operation protocol as its system
                // prompt (so it never forgets to answer with a ```lua block);
                // a plain chat tab gets the friendly chat system prompt.
                let mut system = if brain {
                    crate::i18n::tp(
                        "agent.browser.model.system",
                        &[("br", conn.drives.as_deref().unwrap_or_default())],
                    )
                } else {
                    crate::i18n::t("agent.model.chat_system")
                };
                if let Some(p) = &conn.persona {
                    system.push('\n');
                    system.push_str(p);
                }
                msgs.push(serde_json::json!({"role": "system", "content": system}));
                for (is_user, content) in h.iter() {
                    msgs.push(serde_json::json!({
                        "role": if *is_user { "user" } else { "assistant" },
                        "content": content,
                    }));
                }
                msgs
            };
            match crate::bridge::complete_messages(&conn.url, &conn.model, &conn.headers, &msgs) {
                Ok(reply) => {
                    history
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push((false, reply.clone()));
                    // Stash the verbatim reply BEFORE injecting, so it's ready
                    // by the time DONE fires and on_done reads tab.reply.
                    *last_reply.lock().unwrap_or_else(|e| e.into_inner()) = Some(reply.clone());
                    inject(&format!("{}\r\n", reply.replace('\n', "\r\n")));
                }
                Err(e) => inject(&format!(
                    "\r\n\x1b[31m{}\x1b[0m\r\n",
                    crate::i18n::tp("agent.model.error", &[("e", &e.to_string())])
                )),
            }
            busy.store(false, Ordering::Relaxed);
        });
    }

    /// Mark the start of the model's turn (the same record as a submit in
    /// write_bytes).
    /// This puts detection into "waiting for response," so a DONE on the
    /// injected response fires on_done.
    /// Without this, DONE gets ignored with prompted=false and the discussion never proceeds
    fn mark_turn_start(&self) {
        self.prompted.store(true, Ordering::Relaxed);
        self.submitted_output
            .store(self.output_count(), Ordering::Relaxed);
        self.response_marker
            .store(self.line_position() as u64 + 1, Ordering::Relaxed);
        self.submitted_screen
            .store(self.screen_fingerprint(), Ordering::Relaxed);
        self.saw_working.store(false, Ordering::Relaxed);
        self.submit_tick_ms.store(u64::MAX, Ordering::Relaxed);
        self.resized_while_waiting.store(false, Ordering::Relaxed);
        *self.submitted_rows.lock().unwrap() = self.visible_rows();
    }

    /// The model's turn: hit complete() on a thread, inject the response
    /// into the screen, and write it to say.txt.
    /// Since parser/bytes_out are Arc-shared, the main loop's detection
    /// (BUSY→DONE→on_done) works unchanged. The blocking HTTP call runs on a
    /// separate thread so it never stalls the main loop
    pub fn dispatch_model(&self, prompt: String) {
        let Some(conn) = self.model.clone() else {
            return;
        };
        // Without recording the turn start, the DONE on the injected response would be ignored with prompted=false
        self.mark_turn_start();
        let parser = Arc::clone(&self.parser);
        let counter = Arc::clone(&self.bytes_out);
        // Raised for the whole turn, exactly as a chat turn does it. Both are
        // "this pane is waiting on the API"; only who asked differs.
        let busy = Arc::clone(&self.model_busy);
        busy.store(true, Ordering::Relaxed);
        std::thread::spawn(move || {
            let inject = |s: &str| Self::inject_into(&parser, &counter, s);
            inject(&format!(
                "\r\n\x1b[36m… {}\x1b[0m\r\n",
                crate::i18n::tp("agent.model.generating", &[("model", &conn.model)])
            ));
            // The debate prompt has CLI-oriented instructions mixed in, like
            // "write to say.txt."
            // With the bridge, SHIKISHA does the writing, so we tell the
            // model to "just state your opinion."
            // Since it's stateless, attach the stance (persona) to `system` every time too, so it's never forgotten
            let mut system = crate::i18n::t("agent.model.system");
            if let Some(p) = &conn.persona {
                system.push('\n');
                system.push_str(&crate::i18n::t("agent.model.persona_head"));
                system.push_str(p);
                system.push('\n');
                system.push_str(&crate::i18n::t("agent.model.persona_tail"));
            }
            match crate::bridge::complete(&conn.url, &conn.model, &conn.headers, Some(&system), prompt.trim()) {
                Ok(text) => {
                    if let Some(say) = crate::bridge::extract_say(&prompt) {
                        match std::fs::write(&say, &text) {
                            Ok(_) => inject(&format!(
                                "\x1b[32m→ {}\x1b[0m\r\n",
                                crate::i18n::tp(
                                    "agent.model.wrote",
                                    &[("n", &text.chars().count().to_string())]
                                )
                            )),
                            Err(e) => inject(&format!(
                                "\x1b[31m{}\x1b[0m\r\n",
                                crate::i18n::tp("agent.model.say_failed", &[("e", &e.to_string())])
                            )),
                        }
                    }
                    inject(&format!("{}\r\n", text.replace('\n', "\r\n")));
                }
                Err(e) => inject(&format!(
                    "\x1b[31m{}\x1b[0m\r\n",
                    crate::i18n::tp("agent.model.error", &[("e", &e.to_string())])
                )),
            }
            busy.store(false, Ordering::Relaxed);
        });
    }

    /// Cumulative bytes read from the PTY. Used to check for activity since a given point in time
    pub fn output_count(&self) -> u64 {
        self.bytes_out.load(Ordering::Relaxed)
    }

    /// Elapsed milliseconds since this session was created
    pub fn age_ms(&self) -> u64 {
        self.created.elapsed().as_millis() as u64
    }

    /// Whether it's OK to send automation (on_start) right after startup.
    ///
    /// An AI CLI doesn't accept input until it launches and finishes drawing
    /// the input box. We treat "output appeared and the screen settled" as
    /// "ready." A timeout is also set for programs that never output anything.
    ///
    /// Settling is asked of the screen itself, not inferred from the state.
    /// "Not BUSY" reads as settled only once the state machine has actually
    /// seen the boot output; on the very first ticks of a tab's life it still
    /// holds its birth value, and taking that for calm sent the persona into a
    /// CLI that was mid-launch. `now_ms` comes from the same clock `tick` uses.
    pub fn ready_for_startup_hook(&self, now_ms: u64) -> bool {
        const GIVE_UP_MS: u64 = 15_000;
        /// How long the screen must hold still before we call the launch finished
        const SETTLE_MS: u64 = 700;
        if self.age_ms() > GIVE_UP_MS {
            return true;
        }
        self.had_output()
            // A CLI standing at its own onboarding question (trust this
            // directory? sign in?) is holding still, but it is holding still
            // waiting for a person. Pasting a briefing into it answers the
            // question with the briefing.
            && !matches!(self.state, TabState::Busy | TabState::Question)
            && self.ms_since_change(now_ms) >= SETTLE_MS
    }

    /// An estimate of the number of rows written so far.
    ///
    /// Counting only rows that scrolled off would stay at 0 the whole time
    /// content still fits on screen, leaving no way to tell "where writing
    /// started." Add the on-screen cursor position so the value keeps
    /// increasing as output progresses
    pub fn line_position(&self) -> usize {
        let mut p = self.parser.lock().unwrap_or_else(|e| e.into_inner());
        let saved = p.screen().scrollback();
        p.screen_mut().set_scrollback(usize::MAX / 2);
        let scrolled = p.screen().scrollback();
        p.screen_mut().set_scrollback(saved);
        let (row, _) = p.screen().cursor_position();
        scrolled + row as usize
    }

    /// Turn new output since the marker into text
    /// An entry point for tests to peek directly at the extraction
    #[cfg(test)]
    pub fn capture_for_probe(&self) -> String {
        self.capture_since_marker()
    }

    /// The current visible screen, row by row from the top
    fn visible_rows(&self) -> Vec<String> {
        let p = self.parser.lock().unwrap_or_else(|e| e.into_inner());
        let screen = p.screen();
        let (rows, cols) = screen.size();
        screen.rows(0, cols).take(rows as usize).collect()
    }

    /// Return the range of rows that changed since execution (row numbers from the top, lo..hi).
    ///
    /// What's compared is the screen captured by hand at the moment of
    /// execution, so nothing is hardcoded on wording. It works unchanged
    /// even if the CLI changes its appearance, whether in Japanese or English.
    /// The top edge strips the startup banner, the bottom edge strips the input box frame.
    ///
    /// Working inward from each edge, stop at the first mismatch. The middle
    /// is never touched, so even if a row identical to a pre-execution row
    /// happens to appear inside the answer, no hole opens up
    pub fn changed_span(before: &[String], now: &[String]) -> (usize, usize) {
        let same = |a: &String, b: &String| a.trim_end() == b.trim_end();
        let head = now.iter().zip(before).take_while(|(a, b)| same(a, b)).count();
        if head >= now.len() {
            // Not a single thing changed = the answer hasn't arrived
            return (0, 0);
        }
        // If the row counts differ, the bottom edges no longer correspond, so decide from the top edge alone in that case
        let tail = if before.len() == now.len() {
            now.iter()
                .rev()
                .zip(before.iter().rev())
                .take_while(|(a, b)| same(a, b))
                .count()
        } else {
            0
        };
        (head, now.len().saturating_sub(tail).max(head))
    }

    /// How many rows at the bottom edge form a "pinned frame" that hasn't
    /// moved since the moment of execution.
    ///
    /// Anything below the cursor is known to be frame by position alone.
    /// What's inside that is compared against the pre-execution screen. The
    /// input box itself (the cursor row) always mismatches because its
    /// content is cleared by execution, but that's not the answer, so it must not halt the scan.
    ///
    /// Codex shows a placeholder example on the cursor row ("Implement
    /// {feature}"). Even for a peer that scrolls, the frame stays pinned, so the same logic strips it out
    fn pinned_rows(&self, rows: u16, cols: u16, cursor_row: u16, floor: usize) -> usize {
        let keep = (rows as usize).saturating_sub(floor);
        let mut now: Vec<String> = {
            let p = self.parser.lock().unwrap_or_else(|e| e.into_inner());
            p.screen().rows(0, cols).take(keep).collect()
        };
        let mut before = self.submitted_rows.lock().unwrap().clone();
        before.truncate(keep);
        if let (Some(a), Some(b)) = (
            now.get_mut(cursor_row as usize),
            before.get(cursor_row as usize),
        ) {
            *a = b.clone();
        }
        let (_, end) = Self::changed_span(&before, &now);
        // end = the bottom edge of the changed rows. Below that is the pinned frame
        (rows as usize).saturating_sub(end).max(floor)
    }

    fn capture_since_marker(&self) -> String {
        let p = self.parser.lock().unwrap_or_else(|e| e.into_inner());
        let (rows, cols) = p.screen().size();
        // The cursor sits inside the input box. Below it is frame, not the
        // answer. That's where hint rows (e.g. "Use /skills to list
        // available skills") and status rows (e.g. "gpt-5.5 medium
        // D:\\Test") live
        let (cursor_row, _) = p.screen().cursor_position();
        if p.screen().alternate_screen() {
            // A full-screen TUI doesn't scroll, so fall back to a snapshot
            // of the visible screen.
            // However, anything that was already on screen before execution (e.g. the startup banner) is not the answer
            let (floor, _) = capture_range(rows, cursor_row, 0);
            let keep = (rows as usize).saturating_sub(floor);
            let now: Vec<String> = p.screen().rows(0, cols).take(keep).collect();
            let mut before = self.submitted_rows.lock().unwrap().clone();
            before.truncate(keep);
            // The top edge strips out what was already on screen before execution (the startup banner)
            let (start, _) = Self::changed_span(&before, &now);
            drop(p);
            let lo = self.pinned_rows(rows, cols, cursor_row, floor);
            let hi = (rows as usize).saturating_sub(1).saturating_sub(start);
            if lo > hi {
                return String::new();
            }
            let mut p = self.parser.lock().unwrap_or_else(|e| e.into_inner());
            let text = extract_text(&mut p, lo, hi, cols);
            return text.trim_end().to_string();
        }
        drop(p);

        // Take only the rows written since execution.
        // Adding the screen height here would always mix in a full screen's
        // worth (startup banner, input box), handing back frame instead of the answer
        let stored = self.response_marker.load(Ordering::Relaxed);
        let since = if stored == u64::MAX {
            rows.saturating_sub(1) as usize
        } else {
            self.line_position().saturating_sub(stored as usize)
        };
        let (floor, hi) = capture_range(rows, cursor_row, since);
        // The bottom-edge frame stays pinned even after scrolling
        let lo = self.pinned_rows(rows, cols, cursor_row, floor);
        if lo > hi {
            return String::new();
        }
        let mut p = self.parser.lock().unwrap_or_else(|e| e.into_inner());
        let text = extract_text(&mut p, lo, hi, cols);
        text.trim_end().to_string()
    }
}



#[cfg(test)]
mod real_codex_probe {
    use super::{Tab, TabOptions};
    use std::io::Write as _;
    use std::time::{Duration, Instant};

    /// Paste into a real Codex and execute, writing out everything that happens.
    ///
    ///   cargo test probe_real_codex -- --ignored --nocapture
    ///
    /// Substituting a fake would only confirm "the behavior we assume,"
    /// so observe it on the real thing
    #[test]
    #[ignore]
    fn probe_real_codex() {
        let dir = std::env::temp_dir().join("shikisha-codex-probe");
        let _ = std::fs::create_dir_all(&dir);
        let out_path = dir.join("probe.txt");
        let mut log = std::fs::File::create(&out_path).unwrap();

        let argv = vec!["codex".to_string()];
        let opts = TabOptions {
            cwd: Some(dir.clone()),
            ..TabOptions::default()
        };
        let mut t = Tab::spawn("codex".into(), &argv, Some("codex".into()), 30, 100, opts).unwrap();
        let start = Instant::now();

        let snap = |t: &mut Tab, log: &mut std::fs::File, phase: &str| {
            let (old, new) = t.tick(start);
            let screen = super::visible_text(t.parser.lock().unwrap_or_else(|e| e.into_inner()).screen());
            let tail: Vec<&str> = screen
                .lines()
                .filter(|l| !l.trim().is_empty())
                .rev()
                .take(6)
                .collect();
            let _ = writeln!(
                log,
                "[{:>6}ms] {phase} 状態={}->{} prompted={} working見た={} マッチ={:?} 応答あり={} 出力={}\n    画面末尾: {:?}",
                start.elapsed().as_millis(),
                old.label(),
                new.label(),
                t.was_prompted(),
                t.saw_working_flag(),
                t.detector.working_matched(),
                t.answered_since_submit(),
                t.output_count(),
                tail
            );
        };

        // Wait for startup. Answer once the trust prompt appears
        let mut trusted = false;
        for _ in 0..80 {
            std::thread::sleep(Duration::from_millis(200));
            snap(&mut t, &mut log, "起動中");
            let screen = super::visible_text(t.parser.lock().unwrap_or_else(|e| e.into_inner()).screen());
            if !trusted && screen.contains("Do you trust") {
                let _ = writeln!(log, "=== 信頼確認に 1 を返す ===");
                t.write_bytes(b"1\r").unwrap();
                trusted = true;
            }
            if trusted && screen.contains("Pasted") {
                break;
            }
        }
        // Wait until it settles
        for _ in 0..30 {
            std::thread::sleep(Duration::from_millis(200));
            snap(&mut t, &mut log, "待機中");
        }
        let _ = writeln!(log, "=== 待機時の画面全体 ===\n{}",
                         super::visible_text(t.parser.lock().unwrap_or_else(|e| e.into_inner()).screen()));

        // Paste roughly the same length as the user's case
        let body = format!(
            "これはテストです。返事は OK の一言だけにしてください。{}",
            "あ".repeat(1900)
        );
        let bracketed = t.parser.lock().unwrap_or_else(|e| e.into_inner()).screen().bracketed_paste();
        let _ = writeln!(log, "=== 貼り付け ({}文字) 括弧付き貼り付け={} ===", body.chars().count(), bracketed);
        let mut bytes = Vec::new();
        if bracketed {
            bytes.extend_from_slice(b"\x1b[200~");
            bytes.extend_from_slice(body.as_bytes());
            bytes.extend_from_slice(b"\x1b[201~");
        } else {
            bytes.extend_from_slice(body.as_bytes());
        }
        t.write_bytes(&bytes).unwrap();

        // Wait until the paste ingestion "finishes" (not until it starts)
        let mut last = t.output_count();
        let mut quiet = 0;
        for _ in 0..100 {
            std::thread::sleep(Duration::from_millis(100));
            snap(&mut t, &mut log, "貼付後");
            let now = t.output_count();
            if now == last {
                quiet += 1;
                if quiet >= 4 {
                    break;
                }
            } else {
                quiet = 0;
                last = now;
            }
        }
        let _ = writeln!(log, "=== 貼り付けの取り込みが落ち着いた (出力={}) ===", t.output_count());

        let _ = writeln!(log, "=== 実行 (Enter) ===");
        t.write_bytes(b"\r").unwrap();

        // Follow the situation for a while after execution
        for _ in 0..150 {
            std::thread::sleep(Duration::from_millis(200));
            snap(&mut t, &mut log, "実行後");
        }

        let _ = writeln!(log, "=== 最終画面 ===\n{}", super::visible_text(t.parser.lock().unwrap_or_else(|e| e.into_inner()).screen()));
        let _ = writeln!(log, "=== 取り込んだ応答 ===\n{:?}", t.last_response);
        t.kill();
        println!("書き出し: {}", out_path.display());
    }
}

#[cfg(test)]
mod layout_probe {
    use super::{Tab, TabOptions};
    use std::time::{Duration, Instant};

    /// Write out the shape of the input box right after startup, with row numbers and cursor position.
    ///
    ///   cargo test layout_probe -- --ignored --nocapture
    ///
    /// Confirm on the real thing whether "stripping below the cursor" is
    /// enough, or whether the top edge of the frame remains. Adding
    /// adjustments based on guesswork just leads to another fix that doesn't work
    #[test]
    #[ignore]
    fn probe_real_input_box_layout() {
        for cmd in ["codex", "claude"] {
            println!("\n================ {cmd} ================");
            let tab = match Tab::spawn(
                cmd.to_string(),
                &[cmd.to_string()],
                None,
                24,
                100,
                TabOptions::default(),
            ) {
                Ok(t) => t,
                Err(e) => {
                    println!("起動できず: {e}");
                    continue;
                }
            };
            // Wait until the frame finishes drawing (drawing is done once output stops)
            let start = Instant::now();
            let mut last = 0u64;
            let mut quiet = Instant::now();
            while start.elapsed() < Duration::from_secs(40) {
                std::thread::sleep(Duration::from_millis(200));
                let now = tab.output_count();
                if now != last {
                    last = now;
                    quiet = Instant::now();
                } else if last > 0 && quiet.elapsed() > Duration::from_secs(3) {
                    break;
                }
            }

            let p = tab.parser.lock().unwrap_or_else(|e| e.into_inner());
            let screen = p.screen();
            let (rows, cols) = screen.size();
            let (cur_row, cur_col) = screen.cursor_position();
            println!("画面 {rows}行 x {cols}桁 / カーソル row={cur_row} col={cur_col}");
            println!("alternate_screen = {}", screen.alternate_screen());
            println!("カーソルより下: {} 行", rows - 1 - cur_row);
            println!("--- 全 {rows} 行 (深さ: 内容) ---");
            for r in (0..rows).rev() {
                let line = screen.rows(0, cols).nth(r as usize).unwrap_or_default();
                let depth = rows - 1 - r;
                let mark = if r == cur_row { " <== カーソル" } else { "" };
                println!("深さ{depth:>2} | {}{mark}", line.trim_end());
            }
        }
    }
}

#[cfg(test)]
mod capture_probe {
    use super::{Tab, TabOptions};
    use std::time::{Duration, Instant};

    /// Ask a real one a short question, and write out the extraction result verbatim, character by character.
    ///
    ///   cargo test capture_probe -- --ignored --nocapture
    ///
    /// What we want to know is "where in the range does the answer's body actually start."
    /// If the head is missing by the height of the frame, trimming just the bottom edge isn't enough
    #[test]
    #[ignore]
    fn probe_what_the_capture_actually_grabs() {
        let tab = Tab::spawn(
            "claude".into(),
            &["claude".to_string()],
            None,
            24,
            100,
            TabOptions::default(),
        )
        .expect("起動");

        // Wait until the frame finishes drawing
        let quiet_for = |tab: &Tab, ms: u64, cap: u64| {
            let start = Instant::now();
            let mut last = 0u64;
            let mut quiet = Instant::now();
            while start.elapsed() < Duration::from_secs(cap) {
                std::thread::sleep(Duration::from_millis(200));
                let now = tab.output_count();
                if now != last {
                    last = now;
                    quiet = Instant::now();
                } else if last > 0 && quiet.elapsed() > Duration::from_millis(ms) {
                    return true;
                }
            }
            false
        };
        assert!(quiet_for(&tab, 3000, 60), "起動しない");

        // Enter via bracketed paste, then execute once it settles (same order as production)
        let q = "Reply with exactly three lines: AAA then BBB then CCC. Nothing else.";
        tab.write_passthrough(b"\x1b[200~").unwrap();
        tab.write_passthrough(q.as_bytes()).unwrap();
        tab.write_passthrough(b"\x1b[201~").unwrap();
        assert!(quiet_for(&tab, 600, 20), "貼り付けが落ち着かない");

        println!("=== 実行の直前 ===");
        {
            let p = tab.parser.lock().unwrap_or_else(|e| e.into_inner());
            let (rows, _) = p.screen().size();
            let (cur, _) = p.screen().cursor_position();
            println!("rows={rows} cursor_row={cur} below={}", rows - 1 - cur);
        }
        println!("line_position = {}", tab.line_position());

        tab.write_bytes(b"\r").unwrap();
        assert!(quiet_for(&tab, 5000, 120), "答えが返らない");

        println!("=== 実行の直後 ===");
        {
            let p = tab.parser.lock().unwrap_or_else(|e| e.into_inner());
            let (rows, _) = p.screen().size();
            let (cur, _) = p.screen().cursor_position();
            println!("rows={rows} cursor_row={cur} below={}", rows - 1 - cur);
        }
        println!("line_position = {}", tab.line_position());

        let got = tab.capture_for_probe();
        println!("\n=== 切り出し結果 ({} 行) ===", got.lines().count());
        for (i, l) in got.lines().enumerate() {
            println!("{i:>2} | {l}");
        }
        println!("=== ここまで ===");
        println!("AAA を含む: {}", got.contains("AAA"));
        println!("CCC を含む: {}", got.contains("CCC"));
    }
}

#[cfg(test)]
mod turns_probe {
    use super::{Tab, TabOptions};
    use std::time::{Duration, Instant};

    /// What's handed over as the answer must be the answer, and only the
    /// answer. Confirm this on a real terminal.
    ///
    /// What the user saw was the phrase "as your faction, refute the other
    /// side in under 100 characters" tacked on by itself at the start of the
    /// text handed to the other side. It was the back half of a wrapped instruction.
    ///
    /// Since execution is the action that finishes writing that row, using
    /// the cursor row as the starting point pulls in the last line of the
    /// instruction. This only shows up from the 2nd round onward, so a
    /// single-shot test won't catch it
    #[test]
    fn the_instruction_is_not_sent_back_as_part_of_the_answer() {
        let mut tab = Tab::spawn(
            "cmd".into(),
            &["cmd.exe".to_string()],
            None,
            24,
            100,
            TabOptions::default(),
        )
        .expect("起動");

        let settle = |tab: &Tab| {
            let start = Instant::now();
            let mut last = 0u64;
            let mut quiet = Instant::now();
            while start.elapsed() < Duration::from_secs(20) {
                std::thread::sleep(Duration::from_millis(100));
                let now = tab.output_count();
                if now != last {
                    last = now;
                    quiet = Instant::now();
                } else if last > 0 && quiet.elapsed() > Duration::from_millis(700) {
                    return;
                }
            }
        };
        settle(&tab);

        // Make it long enough to wrap past 100 columns (same nature as the user's prompt)
        let long = |n: usize| {
            format!(
                "echo TURN{n}-ドラクエ５のビアンカを妻にすべきかフローラを妻にすべきか議論をしてもらいますあなたはビアンカ派閥として相手を１００文字以内で論破してください-END{n}"
            )
        };

        for turn in 1..=3 {
            let text = long(turn);
            tab.write_passthrough(b"\x1b[200~").unwrap();
            tab.write_passthrough(text.as_bytes()).unwrap();
            tab.write_passthrough(b"\x1b[201~").unwrap();
            settle(&tab);

            let marker_before = tab.line_position();
            tab.write_bytes(b"\r").unwrap();
            settle(&tab);

            let got = tab.capture_for_probe();
            let _ = marker_before;
            assert!(
                got.contains(&format!("TURN{turn}-")) && got.contains(&format!("END{turn}")),
                "{turn}回目の答えが丸ごと入っていない: {got:?}"
            );
            assert!(
                got.trim_start().starts_with(&format!("TURN{turn}-")),
                "指示の折り返しの後半が頭に付いている: {got:?}"
            );
            for prev in 1..turn {
                assert!(
                    !got.contains(&format!("END{prev}")),
                    "{prev}回目の残りを拾っている: {got:?}"
                );
            }
            tab.finish_response();
        }
    }
}

#[cfg(test)]
mod paste_no_submit_probe {
    use super::{Tab, TabOptions};
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};

    /// Sending only a bracketed paste must land in the input box and stop there.
    ///
    /// The foundation for building, from automation, "just insert a draft
    /// and let the human decide whether to send." Observed: short bodies
    /// display as-is; long ones collapse into [Pasted text #1 +N lines]
    ///
    ///   cargo test paste_no_submit -- --ignored --nocapture
    ///
    /// No API is called, so there's no cost. Check on the real thing whether it merely stops after pasting
    #[test]
    #[ignore]
    fn probe_paste_without_enter_stays_unsent() {
        let tab = Tab::spawn("claude".into(), &["claude".to_string()], None, 24, 100,
                             TabOptions::default()).expect("起動");
        let settle = |ms: u64, cap: u64| {
            let start = Instant::now();
            let (mut last, mut quiet) = (0u64, Instant::now());
            while start.elapsed() < Duration::from_secs(cap) {
                std::thread::sleep(Duration::from_millis(200));
                let n = tab.output_count();
                if n != last { last = n; quiet = Instant::now(); }
                else if last > 0 && quiet.elapsed() > Duration::from_millis(ms) { return; }
            }
        };
        settle(3000, 60);

        // Same shape as what Lua produces: ESC[200~ body ESC[201~
        let body = "lp.html を読んでください。\n\n";
        let payload = format!("\x1b[200~{body}\x1b[201~");
        tab.write_bytes(payload.as_bytes()).unwrap();
        settle(1500, 20);

        // If this becomes true, waiting-for-response starts even though
        // nothing was sent, and on_done fires on an empty swing
        assert!(
            !tab.prompted.load(Ordering::Relaxed),
            "括弧貼り付けだけで送信扱いになっている"
        );
        let p = tab.parser.lock().unwrap_or_else(|e| e.into_inner());
        let (rows, cols) = p.screen().size();
        println!("--- 下から8行 ---");
        for r in rows.saturating_sub(8)..rows {
            let l = p.screen().rows(0, cols).nth(r as usize).unwrap_or_default();
            println!("| {}", l.trim_end());
        }
    }
}

#[cfg(test)]
mod draft_target_tests {
    use super::{Tab, TabOptions};
    use std::time::{Duration, Instant};

    fn settle(tab: &Tab, quiet_ms: u64, cap_s: u64) {
        let start = Instant::now();
        let (mut last, mut quiet) = (0u64, Instant::now());
        while start.elapsed() < Duration::from_secs(cap_s) {
            std::thread::sleep(Duration::from_millis(150));
            let n = tab.output_count();
            if n != last {
                last = n;
                quiet = Instant::now();
            } else if last > 0 && quiet.elapsed() > Duration::from_millis(quiet_ms) {
                return;
            }
        }
    }

    fn spawn(cmd: &str) -> Tab {
        Tab::spawn(
            cmd.into(),
            &[cmd.to_string()],
            None,
            24,
            100,
            TabOptions::default(),
        )
        .expect("起動")
    }

    /// A draft must never be placed into a shell.
    ///
    /// Sent to a peer that doesn't understand the markers, the markers are
    /// ignored and the newline inside becomes a real submit. Observed
    /// (cmd.exe): wrapping `echo HELLO` in the markers and appending a
    /// carriage return executed it immediately.
    ///
    /// This must not be decided by appearance or profile name. By
    /// convention, a supporting app declares ESC[?2004h itself — read that instead
    #[test]
    fn a_shell_is_never_given_a_draft() {
        let tab = spawn("cmd.exe");
        settle(&tab, 700, 15);
        assert!(
            !tab.accepts_bracketed_paste(),
            "シェルを下書きの宛先と見なしている"
        );
    }

    /// A draft must be placeable in an AI CLI.
    ///
    ///   cargo test a_draft_reaches -- --ignored
    ///
    /// Observed: cmd.exe = false / powershell.exe = false / claude = true
    #[test]
    #[ignore]
    fn a_draft_reaches_an_ai_cli() {
        let tab = spawn("claude");
        settle(&tab, 2500, 60);
        assert!(
            tab.accepts_bracketed_paste(),
            "AI CLI が下書きを受け取れないことになっている"
        );
    }
}

#[cfg(test)]
mod resize_survival_tests {
    use super::{Tab, TabOptions};
    use std::time::{Duration, Instant};

    /// The stock vt100 0.16.2 panicked when a resize stranded a full-width
    /// character at the row edge (row.rs `clear_wide` indexing past the end,
    /// screen.rs `text_common` unwrapping a missing continuation cell). The
    /// vendored patch must survive the whole neighborhood of that state —
    /// no catch_unwind here on purpose: a panic IS the failure
    #[test]
    fn vendored_vt100_survives_resize_with_wide_chars() {
        let line = "全角テキストの帯あいうえお漢字カナ混在1２３ｗ日本語";
        for &cols in &[51u16, 50, 34, 33, 21, 7, 3, 2, 1] {
            let mut p = vt100::Parser::new(30, 80, 200);
            for _ in 0..35 {
                p.process(line.as_bytes());
                p.process(b"\r\n");
            }
            // Narrow mid-stream, then keep writing wide chars over the
            // stranded halves, erase across wide boundaries, and add
            // combining characters (the zero-width path)
            p.screen_mut().set_size(30, cols);
            for row in 1..12u16 {
                p.process(format!("\x1b[{row};{cols}H").as_bytes());
                p.process("漢".as_bytes());
                p.process(b"\x1b[K");
                p.process(format!("\x1b[{row};1H").as_bytes());
                p.process("か\u{3099}き\u{3099}".as_bytes());
                p.process(b"\x1b[1K");
            }
            p.process(line.repeat(6).as_bytes());
            // Insert/delete/erase sequences across wide-char boundaries
            // (they reach Row::remove / erase / insert / truncate)
            for row in 1..8u16 {
                p.process(format!("\x1b[{row};1H漢字カナ").as_bytes());
                p.process(b"\x1b[3@");
                p.process(b"\x1b[2P");
                p.process(b"\x1b[4X");
                p.process(format!("\x1b[{row};{cols}H\x1b[1P\x1b[2@").as_bytes());
            }
            // ...and widen again, over the same content
            p.screen_mut().set_size(30, 80);
            p.process(line.as_bytes());
            let _ = p.screen().contents();
        }
    }

    fn settle(tab: &Tab, ms: u64) {
        let start = Instant::now();
        let (mut last, mut quiet) = (0u64, Instant::now());
        while start.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(60));
            let n = tab.output_count();
            if n != last {
                last = n;
                quiet = Instant::now();
            } else if last > 0 && quiet.elapsed() > Duration::from_millis(ms) {
                return;
            }
        }
    }

    /// Narrowing the width must not kill the screen.
    ///
    /// vt100 0.16.2 unwraps on the assumption that "if there's a full-width
    /// character, there's also a next cell," and panics if the right edge
    /// stays full-width after narrowing and a half-width character is
    /// written (24 out of 848 cases in an exhaustive sweep). Since we track
    /// the window width regardless, drawing a frame while outputting
    /// Japanese is a path that's guaranteed to be hit eventually.
    ///
    /// A panicked parser is rebuilt via a full reset. Even if the mutex is
    /// poisoned, it doesn't cascade into death (the poisoned side is recovered and execution continues)
    #[test]
    fn narrowing_the_window_does_not_kill_the_screen() {
        let tab = Tab::spawn(
            "cmd".into(),
            &["cmd.exe".to_string()],
            None,
            8,
            40,
            TabOptions::default(),
        )
        .expect("起動");
        settle(&tab, 500);

        for to in [20u16, 7, 5, 11, 60] {
            // Fill all the way to the right edge with full-width characters
            tab.write_passthrough("あいうえおかきくけこさしすせそたちつてと".as_bytes())
                .unwrap();
            settle(&tab, 300);
            tab.resize(8, to).unwrap();
            // A half-width character right after narrowing was the trigger for the panic.
            // Send a newline too, to clear the row (leaving it would run into the next command)
            tab.write_passthrough(b"x\r").unwrap();
            settle(&tab, 300);

            // If it's still alive, the screen can be read (recovered even from a poisoned mutex)
            let text = {
                let p = tab.parser.lock().unwrap_or_else(|e| e.into_inner());
                crate::tab::visible_text(p.screen())
            };
            assert!(
                text.lines().count() > 0,
                "幅 {to} で画面が読めなくなった"
            );
        }

        // Confirm output is still being received all the way through
        tab.write_passthrough(b"echo ALIVE\r").unwrap();
        settle(&tab, 500);
        let text = {
            let p = tab.parser.lock().unwrap_or_else(|e| e.into_inner());
            crate::tab::visible_text(p.screen())
        };
        assert!(
            text.contains("ALIVE"),
            "縮めた後に読み取りが止まっている: {text:?}"
        );
    }
}

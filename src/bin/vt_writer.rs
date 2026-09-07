//! A stand-in for a program that draws on a terminal, for measuring with.
//!
//! `type` and `cat` are the wrong yardstick for a pseudo console. They write
//! plain text through a handle whose console mode says nothing about escape
//! sequences, so the console is entitled to treat what they send as literal
//! characters — and then both pseudo consoles behave identically, because
//! neither is being asked to do the thing they differ at.
//!
//! A real terminal program says so first. This one sets
//! `ENABLE_VIRTUAL_TERMINAL_PROCESSING` on its own output, exactly as an AI CLI
//! or a progress bar does, and only then writes. What it writes is chosen to be
//! byte-for-byte identical on every run, so two runs differ only in which
//! pseudo console carried them.
//!
//!   vt_writer poured    10000 lines of Japanese, each one new
//!   vt_writer redrawn   500 frames that start from the top-left and colour
//!   vt_writer sequences 2000 kitty-graphics APC blocks and no text at all
//!
//! Used by the `frame_bench` measurement in src/main.rs. It is a measuring
//! instrument, not a feature: nothing in the program calls it.

use std::io::Write as _;

/// 39 characters, the width the published console figures were taken at.
const LINE: &str = "吾輩は猫である。名前はまだ無い。どこで生れたか頓と見当がつかぬ。何でも薄暗いじ";
const END: &str = "SHIKISHA-BURST-END";

fn main() {
    let shape = std::env::args().nth(1).unwrap_or_else(|| "poured".into());
    announce_ourselves_as_a_terminal_program();

    let mut out = Vec::with_capacity(1 << 21);
    match shape.as_str() {
        "poured" => {
            for _ in 0..10_000 {
                out.extend_from_slice(LINE.as_bytes());
                out.extend_from_slice(b"\r\n");
            }
        }
        "redrawn" => {
            for frame in 0..500 {
                out.extend_from_slice(b"\x1b[H");
                for row in 0..20 {
                    out.extend_from_slice(format!("\x1b[3{}m", (frame + row) % 8).as_bytes());
                    out.extend_from_slice(LINE.as_bytes());
                    out.extend_from_slice(b"\x1b[K\r\n");
                }
            }
            out.extend_from_slice(b"\x1b[0m\r\n");
        }
        "sequences" => {
            let blob = "A".repeat(200);
            for _ in 0..2_000 {
                out.extend_from_slice(b"\x1b_Gf=24,s=1,v=1;");
                out.extend_from_slice(blob.as_bytes());
                out.extend_from_slice(b"\x1b\\");
            }
            out.extend_from_slice(b"\r\n");
        }
        // Markers with sequences wedged between them. Nothing here is about
        // speed: it is about whether what comes out the other end is still in
        // the order it went in. Passthrough is reported to lose that ordering
        // (microsoft/terminal#8698), and this program decides what a tab is
        // doing by reading its output in order.
        "ordered" => {
            out.extend_from_slice(b"MARK1\r\n");
            out.extend_from_slice(b"\x1b_Gf=24,s=1,v=1;AAAA\x1b\\");
            out.extend_from_slice(b"MARK2\r\n");
            out.extend_from_slice(b"\x1bP0;1;0q#0;2;0;0;0#0~~@@vv@@~~@@~~$\x1b\\");
            out.extend_from_slice(b"MARK3\r\n");
            out.extend_from_slice(b"\x1b]8;;https://example.com\x1b\\LINK\x1b]8;;\x1b\\");
            out.extend_from_slice(b"MARK4\r\n");
        }
        other => {
            eprintln!("unknown shape {other}");
            std::process::exit(2);
        }
    }
    out.extend_from_slice(END.as_bytes());
    out.extend_from_slice(b"\r\n");

    let mut stdout = std::io::stdout().lock();
    let _ = stdout.write_all(&out);
    let _ = stdout.flush();
}

/// Say that what follows is meant as escape sequences.
///
/// Without this the console may pass the bytes on as characters, and a
/// measurement of "how well does this console carry escape sequences" turns
/// into a measurement of nothing.
#[cfg(windows)]
fn announce_ourselves_as_a_terminal_program() {
    use windows_sys::Win32::System::Console::{
        DISABLE_NEWLINE_AUTO_RETURN, ENABLE_PROCESSED_OUTPUT,
        ENABLE_VIRTUAL_TERMINAL_PROCESSING, GetStdHandle, STD_OUTPUT_HANDLE, SetConsoleMode,
    };
    unsafe {
        let h = GetStdHandle(STD_OUTPUT_HANDLE);
        SetConsoleMode(
            h,
            ENABLE_PROCESSED_OUTPUT
                | ENABLE_VIRTUAL_TERMINAL_PROCESSING
                | DISABLE_NEWLINE_AUTO_RETURN,
        );
    }
}

#[cfg(not(windows))]
fn announce_ourselves_as_a_terminal_program() {}

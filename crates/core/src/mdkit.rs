//! The Markdown reader and editor, carried inside the program.
//!
//! Built once from pinned libraries into one file (tools/mdkit: unified and
//! remark/rehype for reading, Tiptap for the visual editor, KaTeX for maths)
//! plus the diagram library and the maths fonts. Carried for the reason Ace is
//! (see ace.rs): a file travelling beside the program can go missing without a
//! word, and the phone is answered from the same bytes as the window.
//!
//! Nothing here is ours but the few lines that tie the libraries together;
//! their licences travel in THIRD-PARTY-NOTICES.txt.

/// Every file, by the name the page asks for, with what it is
const FILES: &[(&str, &[u8], &str)] = &[
    ("mdkit.js", include_bytes!("../../../vendor/mdkit/mdkit.js"), JS),
    ("mermaid.js", include_bytes!("../../../vendor/mdkit/mermaid.js"), JS),
    ("katex.css", include_bytes!("../../../vendor/mdkit/katex.css"), CSS),
    ("fonts/KaTeX_AMS-Regular.woff2", include_bytes!("../../../vendor/mdkit/fonts/KaTeX_AMS-Regular.woff2"), FONT),
    ("fonts/KaTeX_Caligraphic-Bold.woff2", include_bytes!("../../../vendor/mdkit/fonts/KaTeX_Caligraphic-Bold.woff2"), FONT),
    ("fonts/KaTeX_Caligraphic-Regular.woff2", include_bytes!("../../../vendor/mdkit/fonts/KaTeX_Caligraphic-Regular.woff2"), FONT),
    ("fonts/KaTeX_Fraktur-Bold.woff2", include_bytes!("../../../vendor/mdkit/fonts/KaTeX_Fraktur-Bold.woff2"), FONT),
    ("fonts/KaTeX_Fraktur-Regular.woff2", include_bytes!("../../../vendor/mdkit/fonts/KaTeX_Fraktur-Regular.woff2"), FONT),
    ("fonts/KaTeX_Main-Bold.woff2", include_bytes!("../../../vendor/mdkit/fonts/KaTeX_Main-Bold.woff2"), FONT),
    ("fonts/KaTeX_Main-BoldItalic.woff2", include_bytes!("../../../vendor/mdkit/fonts/KaTeX_Main-BoldItalic.woff2"), FONT),
    ("fonts/KaTeX_Main-Italic.woff2", include_bytes!("../../../vendor/mdkit/fonts/KaTeX_Main-Italic.woff2"), FONT),
    ("fonts/KaTeX_Main-Regular.woff2", include_bytes!("../../../vendor/mdkit/fonts/KaTeX_Main-Regular.woff2"), FONT),
    ("fonts/KaTeX_Math-BoldItalic.woff2", include_bytes!("../../../vendor/mdkit/fonts/KaTeX_Math-BoldItalic.woff2"), FONT),
    ("fonts/KaTeX_Math-Italic.woff2", include_bytes!("../../../vendor/mdkit/fonts/KaTeX_Math-Italic.woff2"), FONT),
    ("fonts/KaTeX_SansSerif-Bold.woff2", include_bytes!("../../../vendor/mdkit/fonts/KaTeX_SansSerif-Bold.woff2"), FONT),
    ("fonts/KaTeX_SansSerif-Italic.woff2", include_bytes!("../../../vendor/mdkit/fonts/KaTeX_SansSerif-Italic.woff2"), FONT),
    ("fonts/KaTeX_SansSerif-Regular.woff2", include_bytes!("../../../vendor/mdkit/fonts/KaTeX_SansSerif-Regular.woff2"), FONT),
    ("fonts/KaTeX_Script-Regular.woff2", include_bytes!("../../../vendor/mdkit/fonts/KaTeX_Script-Regular.woff2"), FONT),
    ("fonts/KaTeX_Size1-Regular.woff2", include_bytes!("../../../vendor/mdkit/fonts/KaTeX_Size1-Regular.woff2"), FONT),
    ("fonts/KaTeX_Size2-Regular.woff2", include_bytes!("../../../vendor/mdkit/fonts/KaTeX_Size2-Regular.woff2"), FONT),
    ("fonts/KaTeX_Size3-Regular.woff2", include_bytes!("../../../vendor/mdkit/fonts/KaTeX_Size3-Regular.woff2"), FONT),
    ("fonts/KaTeX_Size4-Regular.woff2", include_bytes!("../../../vendor/mdkit/fonts/KaTeX_Size4-Regular.woff2"), FONT),
    ("fonts/KaTeX_Typewriter-Regular.woff2", include_bytes!("../../../vendor/mdkit/fonts/KaTeX_Typewriter-Regular.woff2"), FONT),
];

const JS: &str = "application/javascript; charset=utf-8";
const CSS: &str = "text/css; charset=utf-8";
const FONT: &str = "font/woff2";

/// The front of the path these are served under
pub const PREFIX: &str = "/vendor/mdkit/";

/// The bytes for one of them and what kind of file it is, if the path names
/// one. Matched whole against the list: no path is assembled
pub fn asset(path: &str) -> Option<(&'static [u8], &'static str)> {
    let name = path.strip_prefix(PREFIX)?;
    FILES.iter().find(|(n, _, _)| *n == name).map(|(_, b, t)| (*b, *t))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_kit_its_diagrams_and_its_fonts_are_carried() {
        assert!(asset("/vendor/mdkit/mdkit.js").is_some_and(|(b, t)| b.len() > 100_000 && t.starts_with("application/javascript")));
        assert!(asset("/vendor/mdkit/mermaid.js").is_some());
        assert!(asset("/vendor/mdkit/fonts/KaTeX_Main-Regular.woff2").is_some_and(|(_, t)| t == "font/woff2"));
        assert!(asset("/vendor/mdkit/../ace/ace.js").is_none());
        // Every font the stylesheet names is one that is carried
        let css = String::from_utf8_lossy(asset("/vendor/mdkit/katex.css").unwrap().0).to_string();
        for part in css.split("url(").skip(1) {
            let name = part.split(')').next().unwrap_or("");
            assert!(asset(&format!("{PREFIX}{name}")).is_some(), "katex.css names {name}, which is not carried");
        }
    }
}

//! Text that arrives as bytes, and may not be UTF-8.
//!
//! git hands a diff over exactly as the file is written. A CSV saved by a
//! spreadsheet on a Japanese machine is Shift_JIS, and read as UTF-8 every
//! character in it is a replacement mark. Worse than unreadable: a piece of
//! that diff handed back to `git apply` would write the marks into the file.
//!
//! So reading and writing go through here together. A reading says whether it
//! is *exact* -- whether writing the text back gives the very same bytes -- and
//! only an exact reading is something that can be handed back.

use encoding_rs::{Encoding, UTF_8};

/// What a person can pick when the guess is wrong, in the order most people
/// would look for them. Anything else `named` accepts still works from Lua;
/// this is only the short list worth putting in front of somebody
pub const CHOICES: &[&str] = &["UTF-8", "Shift_JIS", "EUC-JP", "GBK", "Big5", "EUC-KR", "windows-1252"];

/// Text, and what it was read as
pub struct Reading {
    pub text: String,
    pub encoding: &'static Encoding,
    /// Writing `text` back in `encoding` gives the bytes it was read from
    pub exact: bool,
}

/// An encoding by any of its names (`sjis`, `cp932` and `Shift_JIS` are one).
///
/// Only encodings a diff can be read in line by line: ASCII stays ASCII in
/// them, so git's own lines around the file's are untouched, and each writes
/// back as itself. UTF-16 fails the first (git calls such a file binary
/// anyway) and ISO-2022-JP the first too, its escapes carrying state across
/// lines that a single hunk cannot
pub fn named(label: &str) -> Option<&'static Encoding> {
    let label = label.trim();
    // Windows' code page numbers, which is what most tools outside a browser
    // call these. The web's list of names leaves some of them out
    let label = match label.to_ascii_lowercase().as_str() {
        "cp932" => "windows-31j",
        "cp936" => "gbk",
        "cp949" => "euc-kr",
        "cp950" => "big5",
        _ => label,
    };
    Encoding::for_label(label.as_bytes()).filter(|e| usable(e))
}

fn usable(e: &'static Encoding) -> bool {
    e.is_ascii_compatible() && e.output_encoding() == e
}

/// The encoding these bytes are most likely written in.
///
/// UTF-8 whenever they are valid UTF-8: legacy text of any length almost never
/// is by accident. Otherwise the detector's guess, nudged toward the language
/// this machine is set up for (the same bytes can be read as Japanese or
/// Chinese, and the machine is the best hint there is). A guess is only taken
/// when it reads the bytes exactly; failing that, UTF-8, which is at least what
/// everything else here would have shown
pub fn guess(bytes: &[u8]) -> &'static Encoding {
    if std::str::from_utf8(bytes).is_ok() {
        return UTF_8;
    }
    let mut detector = chardetng::EncodingDetector::new(chardetng::Iso2022JpDetection::Deny);
    detector.feed(bytes, true);
    let local = crate::discover::legacy_console_encoding().and_then(|(name, _)| named(name));
    let guessed = detector.guess(local.and_then(region_of), chardetng::Utf8Detection::Deny);
    [Some(guessed), local]
        .into_iter()
        .flatten()
        .find(|e| usable(e) && read_as(bytes, e).exact)
        .unwrap_or(UTF_8)
}

/// The top-level domain the detector takes as a hint for a language, for the
/// legacy encodings a Windows machine can be set to
fn region_of(e: &'static Encoding) -> Option<&'static [u8]> {
    match e.name() {
        "Shift_JIS" => Some(b"jp"),
        "GBK" | "gb18030" => Some(b"cn"),
        "EUC-KR" => Some(b"kr"),
        "Big5" => Some(b"tw"),
        _ => None,
    }
}

/// Read the bytes as `encoding`, however well that goes
pub fn read_as(bytes: &[u8], encoding: &'static Encoding) -> Reading {
    let (text, had_errors) = encoding.decode_without_bom_handling(bytes);
    let text = text.into_owned();
    let exact = !had_errors && (encoding == UTF_8 || write_as(&text, encoding).as_deref() == Some(bytes));
    Reading { text, encoding, exact }
}

/// Read the bytes as whatever they most likely are
pub fn read(bytes: &[u8]) -> Reading {
    read_as(bytes, guess(bytes))
}

/// The text as bytes in `encoding`, or `None` when it holds a character that
/// encoding has no way to write. Never a substitute: a `?` in place of a
/// character is a changed file
pub fn write_as(text: &str, encoding: &'static Encoding) -> Option<Vec<u8>> {
    let (bytes, _, unmappable) = encoding.encode(text);
    (!unmappable).then(|| bytes.into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sjis(text: &str) -> Vec<u8> {
        write_as(text, encoding_rs::SHIFT_JIS).unwrap()
    }

    #[test]
    fn a_spreadsheets_japanese_csv_is_read_as_shift_jis() {
        let bytes = sjis("氏名,住所,電話番号\r\n山田太郎,東京都千代田区,03-1234-5678\r\n");
        let r = read(&bytes);
        assert_eq!(r.encoding, encoding_rs::SHIFT_JIS);
        assert!(r.exact);
        assert!(r.text.contains("山田太郎"));
    }

    #[test]
    fn a_short_line_of_japanese_is_still_read_as_japanese() {
        let r = read(&sjis("+商品名,価格\n"));
        assert_eq!(r.encoding, encoding_rs::SHIFT_JIS);
        assert_eq!(r.text, "+商品名,価格\n");
    }

    #[test]
    fn euc_jp_is_told_apart_from_shift_jis() {
        let bytes = write_as("日本語のテキストです。文字コードが違います。\n", encoding_rs::EUC_JP).unwrap();
        let r = read(&bytes);
        assert_eq!(r.encoding, encoding_rs::EUC_JP);
        assert!(r.exact);
    }

    #[test]
    fn utf8_stays_utf8() {
        let r = read("日本語\n".as_bytes());
        assert_eq!(r.encoding, UTF_8);
        assert!(r.exact);
    }

    #[test]
    fn a_wrong_choice_is_not_exact_and_so_cannot_be_written_back() {
        let r = read_as(&sjis("日本語\n"), UTF_8);
        assert!(!r.exact);
        assert!(r.text.contains('\u{FFFD}'));
        assert_eq!(write_as(&r.text, encoding_rs::SHIFT_JIS), None);
    }

    #[test]
    fn names_people_use_are_understood_and_ones_a_diff_cannot_use_are_not() {
        assert_eq!(named("sjis"), Some(encoding_rs::SHIFT_JIS));
        assert_eq!(named(" CP932 "), Some(encoding_rs::SHIFT_JIS));
        assert_eq!(named("windows-1252"), Some(encoding_rs::WINDOWS_1252));
        assert_eq!(named("utf-16le"), None);
        assert_eq!(named("iso-2022-jp"), None);
        assert_eq!(named("no such thing"), None);
        for c in CHOICES {
            assert!(named(c).is_some(), "{c} is offered but not accepted");
        }
    }
}

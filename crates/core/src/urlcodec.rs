//! URL components share bytes, while paths and form queries differ on `+`.

/// Encode a UTF-8 component using the RFC 3986 unreserved set.
pub fn encode(text: &str) -> String {
    encode_with(text, b"", false)
}

pub fn encode_path(text: &str) -> String {
    encode_with(text, b"/", false)
}

pub fn encode_readable_path(text: &str) -> String {
    encode_with(text, b"/:", false)
}

pub fn encode_form(text: &str) -> String {
    encode_with(text, b"", true)
}

fn encode_with(text: &str, keep: &[u8], space_as_plus: bool) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(text.len());
    for b in text.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char)
            }
            b' ' if space_as_plus => out.push('+'),
            b if keep.contains(&b) => out.push(b as char),
            _ => {
                let _ = write!(out, "%{b:02X}");
            }
        }
    }
    out
}

/// Decode escapes once. Incomplete escapes stay literal; UTF-8 policy belongs
/// to the caller, since a terminal link must reject invalid text.
pub fn decode_bytes(text: &str, plus_as_space: bool) -> Vec<u8> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = |b: u8| (b as char).to_digit(16);
            if let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(if plus_as_space && bytes[i] == b'+' {
            b' '
        } else {
            bytes[i]
        });
        i += 1;
    }
    out
}

pub fn decode_path(text: &str) -> String {
    String::from_utf8_lossy(&decode_bytes(text, false)).into_owned()
}

pub fn decode_query(text: &str) -> String {
    String::from_utf8_lossy(&decode_bytes(text, true)).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn url_components_keep_their_meaning() {
        let text = "日本語 /+%?#&=~";
        assert_eq!(decode_path(&encode(text)), text);
        assert_eq!(decode_query(&encode(text)), text);
        assert_eq!(decode_path("a+b%2Bc%2520"), "a+b+c%20");
        assert_eq!(decode_query("a+b%2Bc%2520"), "a b+c%20");
        assert_eq!(decode_path("% %0 %GG"), "% %0 %GG");
        assert!(String::from_utf8(decode_bytes("%FF", false)).is_err());
        assert_eq!(decode_path("%FF"), "\u{fffd}");
        assert_eq!(encode_path("/a b:c+"), "/a%20b%3Ac%2B");
        assert_eq!(encode_readable_path("D:/a b+c?"), "D:/a%20b%2Bc%3F");
        assert_eq!(encode_form("a b+c/"), "a+b%2Bc%2F");
    }
}

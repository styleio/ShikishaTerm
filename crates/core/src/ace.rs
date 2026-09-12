//! The editor's own code, carried inside the program.
//!
//! One prebuilt library (Ace, BSD-3) and the language files for it. Carried
//! rather than laid beside the executable for the reason the manual keeps
//! learning the hard way: a file that travels separately is a file that can be
//! missing, and a feature that is missing a file dies without saying so. It is
//! also how the phone gets it -- the same route, from the same bytes.
//!
//! Nothing here is ours. It is not modified, and its licence travels in
//! THIRD-PARTY-NOTICES.txt with everything else.

/// Every file we carry, by the name the page asks for.
const FILES: &[(&str, &[u8])] = &[
    ("ace.js", include_bytes!("../../../vendor/ace/ace.js")),
    ("ext-searchbox.js", include_bytes!("../../../vendor/ace/ext-searchbox.js")),
    ("mode-c_cpp.js", include_bytes!("../../../vendor/ace/mode-c_cpp.js")),
    ("mode-css.js", include_bytes!("../../../vendor/ace/mode-css.js")),
    ("mode-golang.js", include_bytes!("../../../vendor/ace/mode-golang.js")),
    ("mode-html.js", include_bytes!("../../../vendor/ace/mode-html.js")),
    ("mode-ini.js", include_bytes!("../../../vendor/ace/mode-ini.js")),
    ("mode-java.js", include_bytes!("../../../vendor/ace/mode-java.js")),
    ("mode-javascript.js", include_bytes!("../../../vendor/ace/mode-javascript.js")),
    ("mode-json.js", include_bytes!("../../../vendor/ace/mode-json.js")),
    ("mode-lua.js", include_bytes!("../../../vendor/ace/mode-lua.js")),
    ("mode-markdown.js", include_bytes!("../../../vendor/ace/mode-markdown.js")),
    ("mode-powershell.js", include_bytes!("../../../vendor/ace/mode-powershell.js")),
    ("mode-python.js", include_bytes!("../../../vendor/ace/mode-python.js")),
    ("mode-rust.js", include_bytes!("../../../vendor/ace/mode-rust.js")),
    ("mode-sh.js", include_bytes!("../../../vendor/ace/mode-sh.js")),
    ("mode-sql.js", include_bytes!("../../../vendor/ace/mode-sql.js")),
    ("mode-toml.js", include_bytes!("../../../vendor/ace/mode-toml.js")),
    ("mode-typescript.js", include_bytes!("../../../vendor/ace/mode-typescript.js")),
    ("mode-xml.js", include_bytes!("../../../vendor/ace/mode-xml.js")),
    ("mode-yaml.js", include_bytes!("../../../vendor/ace/mode-yaml.js")),
];

/// The front of the path these are served under. The page asks for
/// `/vendor/ace/ace.js`; nothing else under that name is answered.
pub const PREFIX: &str = "/vendor/ace/";

/// The bytes for one of them, if the path names one.
///
/// The name is matched whole against the list -- no path is assembled, so
/// there is nothing here to walk out of.
pub fn asset(path: &str) -> Option<&'static [u8]> {
    let name = path.strip_prefix(PREFIX)?;
    FILES.iter().find(|(n, _)| *n == name).map(|(_, b)| *b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_library_and_its_languages_are_carried() {
        assert!(asset("/vendor/ace/ace.js").is_some_and(|b| b.len() > 100_000), "本体が入っていない");
        assert!(asset("/vendor/ace/mode-rust.js").is_some(), "言語が入っていない");
    }

    #[test]
    fn nothing_else_is_answered() {
        assert!(asset("/vendor/ace/../../../secrets.json").is_none(), "外へ出られる");
        assert!(asset("/vendor/ace/").is_none());
        assert!(asset("/etc/passwd").is_none());
        assert!(asset("ace.js").is_none(), "接頭辞なしでは答えない");
    }
}

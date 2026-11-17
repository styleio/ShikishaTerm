//! Everything a browser has to say about who it is, once a name is chosen.
//!
//! The name is not the only place the claim is made. It is made again, in
//! `Sec-CH-UA` and in `navigator.userAgentData`, and those are not written
//! from the name -- a browser calling itself Chrome while announcing
//! "Microsoft Edge WebView2" alongside, or calling itself a Mac while
//! announcing Windows, has told a site more than it would have by saying
//! nothing at all. So the rest is built out of the name itself, and they
//! always agree. Every browser this program runs is told its name through
//! here: the window's engines and the server's.

/// What a browser is told to call itself: the name, and everything else that
/// is said beside it, as `Emulation.setUserAgentOverride` takes them.
///
/// A name with no Chromium version in it (someone naming a browser that is
/// not one) is given no brands: that is what a browser which does not speak
/// client hints sends.
pub fn said(ua: &str) -> serde_json::Value {
    let after = |mark: &str| -> Option<String> {
        let at = ua.find(mark)? + mark.len();
        let ver: String = ua[at..].chars().take_while(|c| c.is_ascii_digit()).collect();
        (!ver.is_empty()).then_some(ver)
    };
    let mut brands = Vec::new();
    if let Some(major) = after("Chrome/") {
        // The greased entry is part of the format, not decoration: a site
        // that hard-codes the list is meant to trip over it
        brands.push(serde_json::json!({ "brand": "Chromium", "version": major }));
        brands.push(serde_json::json!({ "brand": "Not=A?Brand", "version": "99" }));
        match after("Edg/") {
            Some(edge) => brands.push(serde_json::json!({ "brand": "Microsoft Edge", "version": edge })),
            None => brands.push(serde_json::json!({ "brand": "Google Chrome", "version": major })),
        }
    }
    let (platform, version, mobile) = platform_of(ua);
    serde_json::json!({
        "userAgent": ua,
        "userAgentMetadata": {
            "brands": brands,
            "platform": platform,
            "platformVersion": version,
            "architecture": architecture_of(ua),
            "bitness": "64",
            "model": "",
            "mobile": mobile,
        },
    })
}

/// The system a name says it runs on, as the hints call it, with the version
/// they give for it and whether it is a phone. Read off the name, so the two
/// cannot disagree: a name that says Macintosh was announced as Windows
fn platform_of(ua: &str) -> (&'static str, &'static str, bool) {
    if ua.contains("Android") {
        ("Android", "", true)
    } else if ua.contains("Windows NT") {
        ("Windows", "15.0.0", false)
    } else if ua.contains("Macintosh") || ua.contains("Mac OS X") {
        ("macOS", "14.0.0", false)
    } else if ua.contains("CrOS") {
        ("Chrome OS", "", false)
    } else if ua.contains("Linux") || ua.contains("X11") {
        ("Linux", "", false)
    } else {
        ("Unknown", "", false)
    }
}

/// The processor a name says it runs on. A Mac's name says "Intel" whatever
/// is inside, so where the name does not say, it is this machine's
fn architecture_of(ua: &str) -> &'static str {
    let names = |words: &[&str]| words.iter().any(|w| ua.contains(w));
    if names(&["arm64", "aarch64", "ARM"]) {
        "arm"
    } else if names(&["x64", "x86_64", "Win64", "WOW64", "amd64"]) {
        "x86"
    } else if cfg!(target_arch = "aarch64") {
        "arm"
    } else {
        "x86"
    }
}

/// The name a browser with no window gives itself, without the word that
/// says it has none: `None` when the name does not say so.
///
/// A browser run with no window calls itself `HeadlessChrome`. Sites read
/// that as a robot and answer it with a refusal, a puzzle, or a page that
/// is not the one a person gets -- while the person watching it from a
/// phone is using it as a browser like any other
pub fn with_a_window(ua: &str) -> Option<String> {
    ua.contains("HeadlessChrome/").then(|| ua.replace("HeadlessChrome/", "Chrome/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A chosen name has to be told the same way twice, or the second telling
    /// gives away the first.
    #[test]
    fn what_the_browser_says_it_is_agrees_with_itself() {
        let chrome = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                      (KHTML, like Gecko) Chrome/151.0.0.0 Safari/537.36";
        let v = said(chrome);
        let brands = v["userAgentMetadata"]["brands"].as_array().unwrap();
        let names: Vec<&str> = brands.iter().filter_map(|b| b["brand"].as_str()).collect();
        assert!(names.contains(&"Google Chrome"), "{names:?}");
        assert!(!names.iter().any(|n| n.contains("WebView")), "it disagrees with what it calls itself: {names:?}");
        assert_eq!(brands[0]["version"], "151");
        assert_eq!(v["userAgentMetadata"]["platform"], "Windows");
        assert_eq!(v["userAgentMetadata"]["architecture"], "x86");

        // Edge names itself twice; both have to be there or the pair is odd
        let edge = format!("{chrome} Edg/151.0.0.0");
        let v = said(&edge);
        let names: Vec<&str> = v["userAgentMetadata"]["brands"].as_array().unwrap().iter().filter_map(|b| b["brand"].as_str()).collect();
        assert!(names.contains(&"Microsoft Edge") && names.contains(&"Chromium"), "{names:?}");

        // Something that is not a Chromium at all sends no brands, which is
        // exactly what such a browser does
        let v = said("Mozilla/5.0 … Firefox/999.0");
        assert!(v["userAgentMetadata"]["brands"].as_array().unwrap().is_empty());
    }

    /// The system beside the name is the one in the name
    #[test]
    fn the_system_it_names_is_the_system_it_announces() {
        let of = |ua: &str| {
            let v = said(ua);
            (v["userAgentMetadata"]["platform"].as_str().unwrap().to_string(), v["userAgentMetadata"]["mobile"].as_bool().unwrap())
        };
        assert_eq!(of("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/151.0.0.0 Safari/537.36"), ("macOS".into(), false), "a Mac announced as something else");
        assert_eq!(of("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/151.0.0.0 Safari/537.36"), ("Linux".into(), false));
        assert_eq!(of("Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/151.0.0.0 Mobile Safari/537.36"), ("Android".into(), true));
        assert_eq!(of("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/151.0.0.0 Safari/537.36"), ("Windows".into(), false));
        assert_eq!(said("Mozilla/5.0 (X11; Linux aarch64) Chrome/151.0.0.0")["userAgentMetadata"]["architecture"], "arm");
    }

    /// A browser with no window is told to call itself what it is with one
    #[test]
    fn a_browser_with_no_window_does_not_say_so() {
        let headless = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) HeadlessChrome/151.0.7000.2 Safari/537.36";
        let plain = with_a_window(headless).expect("left saying it has no window");
        assert_eq!(plain, "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/151.0.7000.2 Safari/537.36");
        let v = said(&plain);
        let names: Vec<&str> = v["userAgentMetadata"]["brands"].as_array().unwrap().iter().filter_map(|b| b["brand"].as_str()).collect();
        assert!(!names.iter().any(|n| n.contains("Headless")), "{names:?}");
        assert_eq!(with_a_window(&plain), None, "a name that does not say so was changed");
    }
}

//! Colours: the sixteen the terminal draws with, and the window around them.
//!
//! A person who cares what their terminal looks like has already chosen a
//! colour scheme somewhere else. On this platform that somewhere is Windows
//! Terminal, whose settings file carries a `schemes` list in a shape the whole
//! world publishes themes in. So the first thing this does is **look there**,
//! and a theme is asked for by the name it already has.
//!
//! Beyond that there are two more places, in the order a person would expect:
//! a `config/themes` folder they can drop any scheme file into, and a handful
//! built in so the list is never empty. Names are matched loosely -- case and
//! spaces are how people misremember a name, not what a name is.
//!
//! What the colours reach is deliberately narrow. The sixteen become CSS
//! variables, so the terminal grid needs to know nothing about themes: it
//! writes `var(--c3)` where it used to write a fixed yellow. The window's own
//! surfaces are **derived** from the scheme's background and foreground rather
//! than being a second thing to configure -- pick a light scheme and the whole
//! window turns light, which is the only behaviour that is not a surprise.

use serde::Deserialize;

/// One colour scheme, in the shape the format is published in.
///
/// Every field optional: a scheme copied from anywhere should work even if it
/// is missing the parts we happen to look at, rather than being refused whole.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Scheme {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub background: Option<String>,
    #[serde(default)]
    pub foreground: Option<String>,
    #[serde(default, rename = "cursorColor")]
    pub cursor: Option<String>,
    #[serde(default, rename = "selectionBackground")]
    pub selection: Option<String>,
    #[serde(default)]
    pub black: Option<String>,
    #[serde(default)]
    pub red: Option<String>,
    #[serde(default)]
    pub green: Option<String>,
    #[serde(default)]
    pub yellow: Option<String>,
    #[serde(default)]
    pub blue: Option<String>,
    #[serde(default)]
    pub purple: Option<String>,
    #[serde(default)]
    pub cyan: Option<String>,
    #[serde(default)]
    pub white: Option<String>,
    #[serde(default, rename = "brightBlack")]
    pub bright_black: Option<String>,
    #[serde(default, rename = "brightRed")]
    pub bright_red: Option<String>,
    #[serde(default, rename = "brightGreen")]
    pub bright_green: Option<String>,
    #[serde(default, rename = "brightYellow")]
    pub bright_yellow: Option<String>,
    #[serde(default, rename = "brightBlue")]
    pub bright_blue: Option<String>,
    #[serde(default, rename = "brightPurple")]
    pub bright_purple: Option<String>,
    #[serde(default, rename = "brightCyan")]
    pub bright_cyan: Option<String>,
    #[serde(default, rename = "brightWhite")]
    pub bright_white: Option<String>,
}

/// The app's own colours, and the scheme every other one falls back to.
///
/// Muted on purpose and leaning toward the logo's blue: a row of pure
/// saturated colours on black reads as a compromised screen, not a tool.
pub const DEFAULT_NAME: &str = "SHIKISHA";

const DEFAULT: [&str; 16] = [
    "#1b2027", "#ff6b6b", "#4ade80", "#ffc857", "#00aaff", "#c792ea", "#4ec9ff", "#c8d2dc",
    "#3a4552", "#ff8f8f", "#7ceaa4", "#ffd88a", "#5cc4ff", "#dcb0ff", "#8fe0ff", "#eef3f8",
];
const DEFAULT_BG: &str = "#0a0c0e";
const DEFAULT_FG: &str = "#e8eef4";

/// The few that ship with the app.
///
/// Not a catalogue -- the catalogue is whatever the person already has. These
/// exist so that the list is never empty and so the well-known names people
/// type from memory resolve to something.
const BUILT_IN: &str = include_str!("themes.json");

impl Scheme {
    /// The sixteen, in the order the terminal numbers them.
    ///
    /// Anything the scheme leaves out keeps ours, so a partial scheme is a
    /// partial change rather than a broken screen.
    pub fn ansi(&self) -> [String; 16] {
        let pick = |v: &Option<String>, fallback: &str| {
            v.as_deref()
                .map(str::trim)
                .filter(|s| is_colour(s))
                .unwrap_or(fallback)
                .to_string()
        };
        [
            pick(&self.black, DEFAULT[0]),
            pick(&self.red, DEFAULT[1]),
            pick(&self.green, DEFAULT[2]),
            pick(&self.yellow, DEFAULT[3]),
            pick(&self.blue, DEFAULT[4]),
            pick(&self.purple, DEFAULT[5]),
            pick(&self.cyan, DEFAULT[6]),
            pick(&self.white, DEFAULT[7]),
            pick(&self.bright_black, DEFAULT[8]),
            pick(&self.bright_red, DEFAULT[9]),
            pick(&self.bright_green, DEFAULT[10]),
            pick(&self.bright_yellow, DEFAULT[11]),
            pick(&self.bright_blue, DEFAULT[12]),
            pick(&self.bright_purple, DEFAULT[13]),
            pick(&self.bright_cyan, DEFAULT[14]),
            pick(&self.bright_white, DEFAULT[15]),
        ]
    }

    fn bg(&self) -> String {
        self.background
            .as_deref()
            .map(str::trim)
            .filter(|s| is_colour(s))
            .unwrap_or(DEFAULT_BG)
            .to_string()
    }

    fn fg(&self) -> String {
        self.foreground
            .as_deref()
            .map(str::trim)
            .filter(|s| is_colour(s))
            .unwrap_or(DEFAULT_FG)
            .to_string()
    }

    /// Everything the page needs, as one block of CSS variables.
    ///
    /// The window's surfaces are mixed from the scheme's own two ends rather
    /// than configured separately. A panel is the background nudged a little
    /// toward the text; a rule is nudged further; dim text is the text pulled
    /// back toward the background. Those proportions hold whichever end is
    /// darker, which is what makes a light scheme simply work.
    pub fn css_vars(&self) -> String {
        let (bg, fg) = (self.bg(), self.fg());
        let ansi = self.ansi();
        let mut out = String::new();
        out.push_str(&format!("--bg:{bg};"));
        out.push_str(&format!("--panel:{};", mix(&bg, &fg, 0.06)));
        out.push_str(&format!("--line:{};", mix(&bg, &fg, 0.16)));
        out.push_str(&format!("--text:{fg};"));
        // Dim is the text pulled back toward the page. Not far: some schemes
        // already sit at a gentle contrast, and taking 45% off those left
        // labels that could not be read
        out.push_str(&format!("--dim:{};", mix(&fg, &bg, 0.35)));
        // Status colours come from the scheme because a scheme's own green and
        // red are the ones chosen to be read against its background. The brand
        // blue does not: it is the app's, not the theme's.
        //
        // The plain eight, never the bright ones. Some well-known schemes put
        // greys in the bright slots on purpose, and a "running" pill that came
        // out grey would say the opposite of what it means
        out.push_str(&format!("--live:{};", ansi[2]));
        out.push_str(&format!("--warn:{};", ansi[3]));
        out.push_str(&format!("--stop:{};", ansi[1]));
        out.push_str("--brand:#00aaff;");
        // The settings screen grew up as its own page and calls three of these
        // by other names. They are the same values, written twice, so that one
        // place still decides what they are and neither page has to be renamed
        // through to say so
        out.push_str(&format!("--muted:{};", mix(&fg, &bg, 0.35)));
        out.push_str("--accent:#00aaff;");
        out.push_str(&format!("--danger:{};", ansi[1]));
        out.push_str(&format!("--panel2:{};", mix(&bg, &fg, 0.11)));
        out.push_str(&format!("--sys:{};", mix(&bg, &fg, 0.13)));
        // Three degrees of surface, because that is what the window actually
        // uses: something under the pointer, something chosen, and something
        // set into the page. Named by what they are for rather than by colour,
        // so they still mean the right thing when the scheme is a light one
        out.push_str(&format!("--hover:{};", mix(&bg, &fg, 0.05)));
        out.push_str(&format!("--raise:{};", mix(&bg, &fg, 0.10)));
        out.push_str(&format!("--sunk:{};", mix(&bg, &fg, 0.03)));
        // A surface that leans toward the accent -- the bar that is loading,
        // the button that is armed
        out.push_str(&format!("--tint:{};", mix(&bg, "#00aaff", 0.16)));
        match self.selection.as_deref().filter(|s| is_colour(s)) {
            Some(sel) => out.push_str(&format!("--sel:{sel};")),
            None => out.push_str(&format!("--sel:{};", mix(&bg, &fg, 0.28))),
        }
        match self.cursor.as_deref().filter(|s| is_colour(s)) {
            Some(cur) => out.push_str(&format!("--cursor:{cur};")),
            None => out.push_str(&format!("--cursor:{fg};")),
        }
        for (i, c) in ansi.iter().enumerate() {
            out.push_str(&format!("--c{i}:{c};"));
        }
        out
    }

    /// The few the settings screen shows as a preview row.
    pub fn swatch(&self) -> Vec<String> {
        let a = self.ansi();
        vec![
            self.bg(),
            self.fg(),
            a[1].clone(),
            a[2].clone(),
            a[3].clone(),
            a[4].clone(),
            a[5].clone(),
            a[6].clone(),
        ]
    }
}

/// `#rgb`, `#rrggbb`, `#rrggbbaa` -- and nothing else, because a colour that
/// is not one has to be refused here rather than reaching the page and
/// silently taking a rule with it.
fn is_colour(s: &str) -> bool {
    let s = s.trim();
    let Some(hex) = s.strip_prefix('#') else {
        return false;
    };
    matches!(hex.len(), 3 | 6 | 8) && hex.chars().all(|c| c.is_ascii_hexdigit())
}

/// `amount` of the way from `a` to `b`.
fn mix(a: &str, b: &str, amount: f32) -> String {
    let (Some(x), Some(y)) = (rgb(a), rgb(b)) else {
        return a.to_string();
    };
    let step = |p: u8, q: u8| (p as f32 + (q as f32 - p as f32) * amount).round() as u8;
    format!(
        "#{:02x}{:02x}{:02x}",
        step(x.0, y.0),
        step(x.1, y.1),
        step(x.2, y.2)
    )
}

fn rgb(s: &str) -> Option<(u8, u8, u8)> {
    let hex = s.trim().strip_prefix('#')?;
    let two = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    let one = |i: usize| u8::from_str_radix(&hex[i..i + 1], 16).ok().map(|v| v * 17);
    match hex.len() {
        3 => Some((one(0)?, one(1)?, one(2)?)),
        6 | 8 => Some((two(0)?, two(2)?, two(4)?)),
        _ => None,
    }
}

/// Whether a scheme is a light one, which is the one thing the page has to be
/// told rather than shown: form controls and scrollbars are drawn by the
/// browser and pick their own colours from this.
pub fn is_light(s: &Scheme) -> bool {
    let Some((r, g, b)) = rgb(&s.bg()) else {
        return false;
    };
    // Rec. 601 luma. Good enough for the one question being asked
    (r as f32 * 0.299 + g as f32 * 0.587 + b as f32 * 0.114) > 128.0
}

/// Every scheme that can be named, nearest source first.
///
/// The order is the order of authority: something the person put in the
/// config folder outranks what another program happens to have, and both
/// outrank ours. Duplicated names keep the first one seen.
pub fn available() -> Vec<Scheme> {
    let mut out: Vec<Scheme> = Vec::new();
    let mut add = |s: Scheme| {
        if s.name.trim().is_empty() {
            return;
        }
        if out.iter().any(|e| loose(&e.name) == loose(&s.name)) {
            return;
        }
        out.push(s);
    };
    for s in from_folder() {
        add(s);
    }
    for s in from_windows_terminal() {
        add(s);
    }
    for s in built_in() {
        add(s);
    }
    out
}

fn built_in() -> Vec<Scheme> {
    serde_json::from_str(BUILT_IN).unwrap_or_default()
}

/// Scheme files a person dropped beside their settings.
///
/// One scheme per file, or a file holding a list of them -- both shapes are
/// published, and asking someone to know which one they have is asking them
/// to open a file they only meant to copy.
fn from_folder() -> Vec<Scheme> {
    let dir = crate::config::root_dir().join("config").join("themes");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for e in entries.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&p) else {
            continue;
        };
        let text = text.trim_start_matches('\u{feff}');
        if let Ok(list) = serde_json::from_str::<Vec<Scheme>>(text) {
            out.extend(list);
        } else if let Ok(mut one) = serde_json::from_str::<Scheme>(text) {
            // A file with no name inside is named by the file, which is how
            // people think of them anyway
            if one.name.trim().is_empty() {
                one.name = p
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
            }
            out.push(one);
        }
    }
    out
}

#[derive(Deserialize)]
struct WtSettings {
    #[serde(default)]
    schemes: Vec<Scheme>,
}

/// The schemes the platform's own terminal already has.
///
/// Read, never written. Its file is that program's, and a theme list is
/// something to borrow rather than take over.
fn from_windows_terminal() -> Vec<Scheme> {
    let Ok(local) = std::env::var("LOCALAPPDATA") else {
        return Vec::new();
    };
    let base = std::path::PathBuf::from(local);
    let places = [
        // Installed from the Store, then the preview build, then unpackaged
        "Packages/Microsoft.WindowsTerminal_8wekyb3d8bbwe/LocalState/settings.json",
        "Packages/Microsoft.WindowsTerminalPreview_8wekyb3d8bbwe/LocalState/settings.json",
        "Microsoft/Windows Terminal/settings.json",
    ];
    for tail in places {
        let p = base.join(tail.replace('/', "\\"));
        let Ok(text) = std::fs::read_to_string(&p) else {
            continue;
        };
        // That file is allowed comments, and has them by default. Stripping
        // them is cheaper than refusing to read the one place worth reading
        let text = strip_comments(text.trim_start_matches('\u{feff}'));
        if let Ok(s) = serde_json::from_str::<WtSettings>(&text) {
            if !s.schemes.is_empty() {
                return s.schemes;
            }
        }
    }
    Vec::new()
}

/// `//` and `/* */`, with strings left alone.
fn strip_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            match c {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                out.push(c);
            }
            '/' if chars.peek() == Some(&'/') => {
                for n in chars.by_ref() {
                    if n == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut last = ' ';
                for n in chars.by_ref() {
                    if last == '*' && n == '/' {
                        break;
                    }
                    last = n;
                }
            }
            _ => out.push(c),
        }
    }
    out
}

/// How a name is compared: the way it is said, not the way it is typed.
fn loose(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// The scheme a setting asks for.
///
/// The setting is either a name to look up or a scheme written out in place,
/// because both are things people have: one is "the theme I use", the other is
/// "these colours I was given".
pub fn resolve(setting: Option<&serde_json::Value>) -> Scheme {
    let fallback = || Scheme {
        name: DEFAULT_NAME.into(),
        ..Default::default()
    };
    let Some(v) = setting else { return fallback() };
    match v {
        serde_json::Value::String(name) if !name.trim().is_empty() => {
            let want = loose(name);
            match available().into_iter().find(|s| loose(&s.name) == want) {
                Some(s) => s,
                None => {
                    crate::append_hook_log(&format!(
                        "no colour scheme called {name:?}; using the built-in one"
                    ));
                    fallback()
                }
            }
        }
        serde_json::Value::Object(_) => match serde_json::from_value::<Scheme>(v.clone()) {
            Ok(mut s) => {
                if s.name.trim().is_empty() {
                    s.name = "custom".into();
                }
                s
            }
            Err(e) => {
                crate::append_hook_log(&format!("the colours in the settings do not read: {e}"));
                fallback()
            }
        },
        _ => fallback(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_built_in_list_reads_and_has_the_app_own_scheme_in_it() {
        let list = built_in();
        assert!(!list.is_empty(), "組み込みのテーマが読めていない");
        assert!(
            list.iter().any(|s| s.name == DEFAULT_NAME),
            "自前のテーマが一覧に無い"
        );
        for s in &list {
            for c in s.ansi() {
                assert!(is_colour(&c), "{} の色がおかしい: {c}", s.name);
            }
        }
    }

    #[test]
    fn a_scheme_that_leaves_things_out_keeps_ours_for_the_rest() {
        let s: Scheme = serde_json::from_str(r##"{"name":"half","red":"#ff0000"}"##).unwrap();
        let a = s.ansi();
        assert_eq!(a[1], "#ff0000");
        assert_eq!(a[2], DEFAULT[2], "書かれていない色は既定のまま");
        assert!(s.css_vars().contains("--c1:#ff0000;"));
    }

    #[test]
    fn a_colour_that_is_not_one_is_refused_rather_than_passed_on() {
        // The danger is a value reaching the page and taking a CSS rule with
        // it -- `red;} body{display:none` inside a variable is a broken window
        let s: Scheme = serde_json::from_str(r##"{"name":"bad","red":"red;}body{x"}"##).unwrap();
        assert_eq!(s.ansi()[1], DEFAULT[1]);
        assert!(!s.css_vars().contains("body{x"));
        assert!(!is_colour("rgb(1,2,3)"));
        assert!(is_colour("#abc") && is_colour("#aabbcc") && is_colour("#aabbccdd"));
    }

    #[test]
    fn the_window_follows_the_scheme_into_the_light() {
        let light: Scheme =
            serde_json::from_str(r##"{"name":"day","background":"#ffffff","foreground":"#202020"}"##)
                .unwrap();
        assert!(is_light(&light));
        let vars = light.css_vars();
        assert!(vars.contains("--bg:#ffffff;"));
        // A panel on a white page is darker than the page, not lighter
        let panel = vars
            .split("--panel:")
            .nth(1)
            .and_then(|s| s.split(';').next())
            .unwrap()
            .to_string();
        let (r, _, _) = rgb(&panel).unwrap();
        assert!(r < 0xff, "明るい配色でパネルが背景より明るい: {panel}");

        let dark: Scheme =
            serde_json::from_str(r##"{"name":"night","background":"#000000"}"##).unwrap();
        assert!(!is_light(&dark));
    }

    #[test]
    fn a_name_is_matched_the_way_it_is_said() {
        assert_eq!(loose("One Half Dark"), loose("onehalfdark"));
        assert_eq!(loose("Catppuccin Mocha"), loose("catppuccin-mocha"));
        assert_ne!(loose("Solarized Dark"), loose("Solarized Light"));
    }

    #[test]
    fn a_setting_can_be_a_name_or_the_colours_themselves() {
        let by_name = resolve(Some(&serde_json::json!(DEFAULT_NAME)));
        assert_eq!(by_name.name, DEFAULT_NAME);
        let inline = resolve(Some(&serde_json::json!({"red":"#123456"})));
        assert_eq!(inline.ansi()[1], "#123456");
        assert_eq!(inline.name, "custom", "名前が無ければそう呼ぶ");
        // A name nobody has is not a reason to come up without colours
        assert_eq!(resolve(Some(&serde_json::json!("nope"))).name, DEFAULT_NAME);
        assert_eq!(resolve(None).name, DEFAULT_NAME);
    }

    #[test]
    fn the_other_terminal_file_is_read_even_with_its_comments_in_it() {
        let text = r##"{
            // the schemes
            "schemes": [ { "name": "X", "red": "#010203" } ], /* and that is all */
            "note": "a // b and /* c */ inside a string stay"
        }"##;
        let s: WtSettings = serde_json::from_str(&strip_comments(text)).unwrap();
        assert_eq!(s.schemes.len(), 1);
        assert_eq!(s.schemes[0].ansi()[1], "#010203");
        assert!(strip_comments(text).contains("a // b and /* c */ inside a string stay"));
    }
}

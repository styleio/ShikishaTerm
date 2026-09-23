//! Keys that work from anywhere on the PC, whatever program is in front: the
//! tools that start from a picture of the screen, and the two things opened
//! over the board -- the quick commands and the ideas.
//!
//! This is the half every platform shares: how a combination is written,
//! which the settings ask for, and what came of asking. Registering them with
//! the system is the window's (`src/hotkeys.rs`), and it reports back here, so
//! the settings page and the board can say what is really in effect.
//!
//! Only Ctrl, Alt and Shift are offered. A combination with the Windows key is
//! the system's to hand out, and an update that claims one takes it from
//! whoever had it with nothing said. A combination another program also uses
//! can at least be seen and changed

/// What a key can be set to open, in the order the settings list them.
/// `snip` is the scissors themselves: take the screen, frame it, then choose
/// the tool. Then the tools by name (see `snip::TOOLS`), and last the two
/// things the board draws over itself, named as the window's own keys name
/// them (`keys::ACTIONS`), so one name means one thing wherever it is written
pub const ACTIONS: &[&str] = &["snip", "text", "noun", "color", "edit", "quick_commands", "ideas"];

/// The actions that open something on the board rather than a tool over a
/// picture of the screen. Pressed from another program, they bring the window
/// forward first
pub const ON_THE_BOARD: &[&str] = &["quick_commands", "ideas"];

/// The combinations set out of the box, by action. Every one is Alt+Shift and
/// a letter, the one pair of held keys the common programs leave alone (see
/// [`DEFAULT`]); the letter is the one the window's own keys use for the
/// same thing. Anything not here starts with no key
pub const DEFAULTS: &[(&str, &str)] =
    &[("snip", DEFAULT), ("quick_commands", "Alt+Shift+K"), ("ideas", "Alt+Shift+M")];

/// The combination an action has out of the box, or "" for none
pub fn default_of(action: &str) -> &'static str {
    DEFAULTS.iter().find(|(a, _)| *a == action).map(|(_, k)| *k).unwrap_or("")
}

/// The actions and their keys out of the box, for the settings page
pub fn catalog_json() -> String {
    let defaults: std::collections::BTreeMap<&str, &str> = DEFAULTS.iter().copied().collect();
    serde_json::json!({ "actions": ACTIONS, "defaults": defaults }).to_string()
}

/// The combination set out of the box for `snip`: two held keys and X, for
/// cutting out, all under the left hand.
///
/// Chosen against the programs nearly everyone has open. Ctrl+Shift with a
/// letter is spent in the browsers, the editors and the office programs almost
/// letter by letter (Ctrl+Shift+X is an editor's extensions list). Ctrl+Alt
/// with a letter is spent in the IDEs, and on keyboards whose AltGr is Ctrl+Alt
/// it is how letters such as € and ł are typed. Alt+Shift+X is not used by
/// the browsers or the common editors; a program that does use it is one the
/// person changes the key for
pub const DEFAULT: &str = "Alt+Shift+X";

/// The system's modifier flags (the values `RegisterHotKey` takes)
pub const MOD_ALT: u32 = 0x1;
pub const MOD_CONTROL: u32 = 0x2;
pub const MOD_SHIFT: u32 = 0x4;

/// One combination: the held keys and the key pressed with them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Combo {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    /// "A"-"Z", "0"-"9" or "F1"-"F12"
    pub key: String,
}

impl Combo {
    /// Read a combination as it is written in the settings. Case, spaces and
    /// the order of the held keys do not matter; what is held has to include
    /// Ctrl or Alt, so typing a capital letter is never taken for a command
    pub fn parse(text: &str) -> Option<Combo> {
        let mut c = Combo { ctrl: false, alt: false, shift: false, key: String::new() };
        for part in text.split('+').map(str::trim) {
            match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => c.ctrl = true,
                "alt" => c.alt = true,
                "shift" => c.shift = true,
                _ if c.key.is_empty() && key_code(part).is_some() => c.key = part.to_ascii_uppercase(),
                _ => return None,
            }
        }
        (!c.key.is_empty() && (c.ctrl || c.alt)).then_some(c)
    }

    /// Written the one way the app writes it: Ctrl, Alt, Shift, then the key
    pub fn shown(&self) -> String {
        let mut parts: Vec<&str> = Vec::new();
        if self.ctrl {
            parts.push("Ctrl");
        }
        if self.alt {
            parts.push("Alt");
        }
        if self.shift {
            parts.push("Shift");
        }
        parts.push(&self.key);
        parts.join("+")
    }

    /// The held keys, as the system's flags
    pub fn mods(&self) -> u32 {
        (if self.ctrl { MOD_CONTROL } else { 0 })
            | (if self.alt { MOD_ALT } else { 0 })
            | (if self.shift { MOD_SHIFT } else { 0 })
    }

    /// The pressed key, as the system's virtual-key code
    pub fn vk(&self) -> u32 {
        key_code(&self.key).unwrap_or(0)
    }
}

/// The virtual-key code of a key a combination may end in.
fn key_code(key: &str) -> Option<u32> {
    let k = key.to_ascii_uppercase();
    let b = k.as_bytes();
    match b {
        [c] if c.is_ascii_uppercase() || c.is_ascii_digit() => Some(*c as u32),
        [b'F', rest @ ..] if !rest.is_empty() => {
            let n: u32 = std::str::from_utf8(rest).ok()?.parse().ok()?;
            (1..=12).contains(&n).then_some(0x70 + n - 1)
        }
        _ => None,
    }
}

/// What the settings ask one action's key to be.
#[derive(Debug, Clone, PartialEq)]
pub enum Wanted {
    /// Nothing: never set, or cleared
    Off,
    Key(Combo),
    /// Written in the settings in a way that is not a combination
    Unreadable(String),
    /// The same combination as an action above it, which keeps it
    Twice(Combo),
}

/// Each action's key as the settings ask for it, in the order of [`ACTIONS`].
/// Not written at all is the key out of the box ([`DEFAULTS`]); written
/// empty, it is off
pub fn wanted(written: &std::collections::BTreeMap<String, String>) -> Vec<(&'static str, Wanted)> {
    let mut taken: Vec<Combo> = Vec::new();
    ACTIONS
        .iter()
        .map(|&action| {
            let text = match written.get(action) {
                Some(t) => t.trim().to_string(),
                None => default_of(action).to_string(),
            };
            let want = if text.is_empty() {
                Wanted::Off
            } else {
                match Combo::parse(&text) {
                    None => Wanted::Unreadable(text),
                    Some(c) if taken.contains(&c) => Wanted::Twice(c),
                    Some(c) => {
                        taken.push(c.clone());
                        Wanted::Key(c)
                    }
                }
            };
            (action, want)
        })
        .collect()
}

/// How one action's key stands, for the settings page and the board.
#[derive(Debug, Clone, serde::Serialize, PartialEq)]
pub struct Row {
    pub action: String,
    /// The combination as the app writes it, or what was written when it
    /// could not be read
    pub key: String,
    /// "on", "off", "taken" (another program has it), "unreadable", "twice"
    pub state: &'static str,
    /// When it last reached this program, in seconds since 1970
    pub last: Option<u64>,
}

static ROWS: std::sync::Mutex<Vec<Row>> = std::sync::Mutex::new(Vec::new());

/// Whether anything on this machine registers keys at all. Only the window
/// does; a runtime serving the board with no window of its own has none
static ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The keys as they were just registered, each with whether it took. Keeps the
/// time a key was last pressed while it stays the same key
pub fn set_registered(rows: Vec<Row>) {
    ACTIVE.store(true, std::sync::atomic::Ordering::Relaxed);
    let mut now = ROWS.lock().unwrap_or_else(|e| e.into_inner());
    let rows = rows
        .into_iter()
        .map(|mut r| {
            if let Some(old) = now.iter().find(|o| o.action == r.action && o.key == r.key) {
                r.last = old.last;
            }
            r
        })
        .collect();
    *now = rows;
}

/// A key reached this program
pub fn pressed(action: &str) {
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .ok();
    let mut now = ROWS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(r) = now.iter_mut().find(|r| r.action == action) {
        r.last = at;
    }
}

/// Every action's key as it stands
pub fn rows() -> Vec<Row> {
    ROWS.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Whether keys are registered by this process at all
pub fn active() -> bool {
    ACTIVE.load(std::sync::atomic::Ordering::Relaxed)
}

/// The keys that work, by action, for the badges beside what they open
pub fn working() -> std::collections::BTreeMap<String, String> {
    rows()
        .into_iter()
        .filter(|r| r.state == "on")
        .map(|r| (r.action, r.key))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A combination is read however it is typed and written back one way
    #[test]
    fn a_combination_is_written_one_way() {
        let c = Combo::parse(" shift + ctrl+alt+x ").unwrap();
        assert_eq!(c.shown(), "Ctrl+Alt+Shift+X");
        assert_eq!(c.mods(), MOD_CONTROL | MOD_ALT | MOD_SHIFT);
        assert_eq!(c.vk(), b'X' as u32);
        assert_eq!(Combo::parse("Alt+F5").unwrap().vk(), 0x74);
        assert_eq!(Combo::parse("Ctrl+7").unwrap().shown(), "Ctrl+7");
    }

    /// What cannot be a combination is refused rather than guessed at: a
    /// letter held with Shift alone is typing, and the Windows key is not
    /// offered at all
    #[test]
    fn what_is_not_a_combination_is_refused() {
        for bad in ["X", "Shift+X", "Win+Shift+X", "Ctrl+Alt", "Ctrl+X+Y", "Ctrl+F13", "Ctrl+Space", ""] {
            assert_eq!(Combo::parse(bad), None, "{bad} was taken as a combination");
        }
    }

    /// The scissors, the quick commands and the ideas have their keys out of
    /// the box; the tools start with none; a key cleared stays cleared; one
    /// key is given to one action only
    #[test]
    fn the_settings_ask_for_one_key_each() {
        let mut written = std::collections::BTreeMap::new();
        let w = wanted(&written);
        let key = |text: &str| Wanted::Key(Combo::parse(text).unwrap());
        assert_eq!(w[0], ("snip", key(DEFAULT)));
        assert!(w[1..5].iter().all(|(_, k)| *k == Wanted::Off), "a tool had a key it was never given");
        assert_eq!(w[5], ("quick_commands", key("Alt+Shift+K")));
        assert_eq!(w[6], ("ideas", key("Alt+Shift+M")));

        written.insert("ideas".to_string(), String::new());
        assert_eq!(wanted(&written)[6].1, Wanted::Off, "the ideas' key came back after it was cleared");

        written.insert("snip".to_string(), String::new());
        written.insert("text".to_string(), "ctrl+alt+shift+t".to_string());
        written.insert("noun".to_string(), "Ctrl+Alt+Shift+T".to_string());
        written.insert("edit".to_string(), "Win+E".to_string());
        let w = wanted(&written);
        assert_eq!(w[0].1, Wanted::Off, "a cleared key came back");
        assert!(matches!(w[1].1, Wanted::Key(_)));
        assert!(matches!(w[2].1, Wanted::Twice(_)), "one key was given to two tools");
        assert_eq!(w[4].1, Wanted::Unreadable("Win+E".into()));
    }

    /// The keys offered are the scissors, the tools the page runs, and the
    /// things on the board -- each by the name the window's keys give it
    #[test]
    fn every_tool_can_have_a_key() {
        assert_eq!(ACTIONS[0], "snip");
        let tools = &ACTIONS[1..ACTIONS.len() - ON_THE_BOARD.len()];
        assert_eq!(tools, crate::snip::TOOLS);
        assert_eq!(&ACTIONS[ACTIONS.len() - ON_THE_BOARD.len()..], ON_THE_BOARD);
        for a in ON_THE_BOARD {
            assert!(crate::keys::char_for(a).is_some(), "{a} is not what the window's keys call it");
        }
    }

    /// Out of the box no two actions share a key, and each is one the
    /// settings would accept if it were written there
    #[test]
    fn the_keys_out_of_the_box_are_keys() {
        let mut seen = Vec::new();
        for (action, text) in DEFAULTS {
            assert!(ACTIONS.contains(action), "{action} has a default but no row");
            let c = Combo::parse(text).unwrap_or_else(|| panic!("{text} is not a combination"));
            assert!(!seen.contains(&c), "{text} is given twice");
            seen.push(c);
        }
    }

    /// Registering the same key again keeps when it was last pressed; a
    /// different key starts with none
    #[test]
    fn the_last_press_is_kept_for_the_same_key() {
        let row = |key: &str| Row { action: "probe-last".into(), key: key.into(), state: "on", last: None };
        set_registered(vec![row("Ctrl+Alt+Q")]);
        pressed("probe-last");
        let first = rows().into_iter().find(|r| r.action == "probe-last").unwrap().last;
        assert!(first.is_some());
        set_registered(vec![row("Ctrl+Alt+Q")]);
        assert_eq!(rows().into_iter().find(|r| r.action == "probe-last").unwrap().last, first);
        set_registered(vec![row("Ctrl+Alt+W")]);
        assert_eq!(rows().into_iter().find(|r| r.action == "probe-last").unwrap().last, None);
        set_registered(Vec::new());
    }
}

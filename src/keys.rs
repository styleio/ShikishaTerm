//! Which key does what, and how a person changes it.
//!
//! The window has always reached its actions through one character after a
//! prefix -- `Ctrl+B %` to split, `Ctrl+B q` to quit -- and the buttons on the
//! page reach the same actions by pretending to press those keys, so that a
//! click and a keypress can never come to mean different things.
//!
//! That character is therefore already the **name** of the action, internally.
//! What was missing was a name a person can say. So this file holds one table:
//! every action, the character it answers to, and a line saying what it does.
//! Everything else reads from it --
//!
//!   - the default keys,
//!   - what a person may rebind, by naming the action rather than the key,
//!   - the help screen, so it shows the keys that are actually in force
//!     rather than the ones we shipped.
//!
//! A rebound key is translated back to the action's own character before
//! anything acts on it. That is deliberate: the window keeps exactly one set
//! of arms to dispatch, the buttons keep working untouched, and remapping
//! cannot quietly break a part of the app that was never told about it.

use std::collections::HashMap;
use std::sync::Mutex;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// One thing the window can be asked to do.
pub struct Action {
    /// What a person calls it in the settings. Never translated -- this is a
    /// handle, and a handle that changes with the display language would make
    /// a settings file stop working when someone switches languages
    pub name: &'static str,
    /// The character the window itself dispatches on. Also what the page's
    /// buttons send
    pub key: char,
    /// Other characters that have always meant the same thing. Kept because
    /// people have muscle memory, not because two spellings are good
    pub also: &'static [char],
    /// Dictionary key for the line shown in the help screen
    pub desc: &'static str,
}

/// Every action, in the order the help screen shows them.
///
/// The order is the order of a working day rather than the alphabet: what you
/// do to tabs, then to panes, then to the workspace, then the rarer things.
pub const ACTIONS: &[Action] = &[
    Action { name: "quit", key: 'q', also: &[], desc: "keys.quit" },
    Action { name: "tab_next", key: 'n', also: &[], desc: "keys.tab_next" },
    Action { name: "tab_prev", key: 'p', also: &[], desc: "keys.tab_prev" },
    Action { name: "add_tab", key: 't', also: &[], desc: "keys.add_tab" },
    Action { name: "restart", key: 'r', also: &[], desc: "keys.restart" },
    Action { name: "restart_fresh", key: 'R', also: &[], desc: "keys.restart_fresh" },
    Action { name: "split_beside", key: '%', also: &['|'], desc: "keys.split_beside" },
    Action { name: "split_below", key: '"', also: &['-'], desc: "keys.split_below" },
    Action { name: "pane_next", key: 'o', also: &[], desc: "keys.pane_next" },
    Action { name: "pane_close", key: 'X', also: &[], desc: "keys.pane_close" },
    Action { name: "panes_even", key: '=', also: &[], desc: "keys.panes_even" },
    Action { name: "divider_left", key: '<', also: &[], desc: "keys.divider_left" },
    Action { name: "divider_right", key: '>', also: &[], desc: "keys.divider_right" },
    Action { name: "workspace_list", key: 'w', also: &[], desc: "keys.workspace_list" },
    Action { name: "workspace_next", key: 'W', also: &[], desc: "keys.workspace_next" },
    Action { name: "copy_mode", key: '[', also: &[], desc: "keys.copy_mode" },
    Action { name: "copy_answer", key: 'c', also: &[], desc: "keys.copy_answer" },
    Action { name: "lock", key: 'l', also: &[], desc: "keys.lock" },
    Action { name: "automation", key: 'a', also: &[], desc: "keys.automation" },
    Action { name: "stop", key: 'x', also: &[], desc: "keys.stop" },
    Action { name: "literal_prefix", key: 'b', also: &[], desc: "keys.literal_prefix" },
    Action { name: "help", key: '?', also: &[], desc: "keys.help" },
    Action { name: "palette", key: ':', also: &[], desc: "keys.palette" },
];

/// The prefix, before anything is read from the settings.
const DEFAULT_PREFIX: &str = "ctrl+b";

/// The prefix in force, kept where the one place that has to *press* it can
/// reach it.
///
/// The page's buttons work by pressing the keys a person would press, so that
/// a click and a keypress can never drift apart. That conversion happens deep
/// inside the window's event plumbing, with no key table in scope -- and if it
/// went on pressing the built-in prefix, moving the prefix would quietly stop
/// every button in the app from working. There is exactly one of these per
/// process, so a process-wide value is what it honestly is.
static IN_FORCE: Mutex<Option<Trigger>> = Mutex::new(None);

/// The prefix a synthesised press should use.
pub fn prefix_now() -> Trigger {
    IN_FORCE
        .lock()
        .ok()
        .and_then(|g| *g)
        .unwrap_or_else(|| Trigger::parse(DEFAULT_PREFIX).expect("the built-in prefix parses"))
}

/// One press: a key and the modifiers held with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Trigger {
    pub code: KeyCode,
    pub mods: KeyModifiers,
}

impl Trigger {
    /// Whether a press is this trigger.
    ///
    /// Shift is not compared when the key is a character, because the shift is
    /// already in the character: a terminal reports `%` for shift+5, and asking
    /// for shift on top of that would mean nothing ever matched
    pub fn matches(&self, ev: &KeyEvent) -> bool {
        if self.code != ev.code {
            return false;
        }
        let mask = match self.code {
            KeyCode::Char(_) => KeyModifiers::CONTROL | KeyModifiers::ALT,
            _ => KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SHIFT,
        };
        (self.mods & mask) == (ev.modifiers & mask)
    }

    /// `ctrl+b`, `ctrl+shift+t`, `alt+f4`, `f5`, `%`.
    ///
    /// Written the way people write keys to each other, because that is what
    /// they will type into the settings
    pub fn parse(text: &str) -> Option<Trigger> {
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        let mut mods = KeyModifiers::NONE;
        let mut rest = text;
        loop {
            let lower = rest.to_ascii_lowercase();
            let taken = ["ctrl+", "control+", "alt+", "opt+", "option+", "shift+"]
                .into_iter()
                .find(|p| lower.starts_with(p));
            let Some(p) = taken else { break };
            mods |= match p {
                "ctrl+" | "control+" => KeyModifiers::CONTROL,
                "shift+" => KeyModifiers::SHIFT,
                _ => KeyModifiers::ALT,
            };
            rest = &rest[p.len()..];
        }
        let mut code = named_code(rest)?;
        // Ctrl+B and Ctrl+b are one key, and people write it both ways. Case
        // is only kept for a bare character, where it really does distinguish
        // two things -- `r` restarts and `R` restarts from nothing
        if let KeyCode::Char(c) = code {
            if mods.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) {
                code = KeyCode::Char(c.to_ascii_lowercase());
            }
        }
        Some(Trigger { code, mods })
    }

    /// How it is written back out, for the help screen and the settings.
    pub fn show(&self) -> String {
        let mut out = String::new();
        if self.mods.contains(KeyModifiers::CONTROL) {
            out.push_str("Ctrl+");
        }
        if self.mods.contains(KeyModifiers::ALT) {
            out.push_str("Alt+");
        }
        if self.mods.contains(KeyModifiers::SHIFT) {
            out.push_str("Shift+");
        }
        match self.code {
            // A control key is written in the case people write it in, which
            // for a letter held with Ctrl is upper
            KeyCode::Char(c) if self.mods.contains(KeyModifiers::CONTROL) => {
                out.extend(c.to_uppercase())
            }
            KeyCode::Char(c) => out.push(c),
            KeyCode::F(n) => out.push_str(&format!("F{n}")),
            other => out.push_str(name_of(other)),
        }
        out
    }
}

fn named_code(text: &str) -> Option<KeyCode> {
    let mut chars = text.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        return Some(KeyCode::Char(c));
    }
    Some(match text.to_ascii_lowercase().as_str() {
        "enter" | "return" => KeyCode::Enter,
        "esc" | "escape" => KeyCode::Esc,
        "tab" => KeyCode::Tab,
        "space" => KeyCode::Char(' '),
        "backspace" | "bs" => KeyCode::Backspace,
        "delete" | "del" => KeyCode::Delete,
        "insert" | "ins" => KeyCode::Insert,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" | "pgup" => KeyCode::PageUp,
        "pagedown" | "pgdn" => KeyCode::PageDown,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        other => {
            let n = other.strip_prefix('f')?.parse::<u8>().ok()?;
            return (1..=12).contains(&n).then_some(KeyCode::F(n));
        }
    })
}

fn name_of(code: KeyCode) -> &'static str {
    match code {
        KeyCode::Enter => "Enter",
        KeyCode::Esc => "Esc",
        KeyCode::Tab => "Tab",
        KeyCode::Backspace => "Backspace",
        KeyCode::Delete => "Delete",
        KeyCode::Insert => "Insert",
        KeyCode::Home => "Home",
        KeyCode::End => "End",
        KeyCode::PageUp => "PageUp",
        KeyCode::PageDown => "PageDown",
        KeyCode::Up => "Up",
        KeyCode::Down => "Down",
        KeyCode::Left => "Left",
        KeyCode::Right => "Right",
        _ => "?",
    }
}

/// The keys in force this run.
pub struct Keys {
    prefix: Trigger,
    /// What a character typed after the prefix really means. Holds the
    /// defaults and their old spellings, minus anything a person moved away
    typed: HashMap<char, char>,
    /// Combos that need no prefix at all
    direct: Vec<(Trigger, char)>,
}

impl Default for Keys {
    fn default() -> Self {
        Keys::from(None, &HashMap::new()).0
    }
}

impl Keys {
    /// Read the settings, and say plainly what could not be used.
    ///
    /// Nothing here is fatal. A key that cannot be understood leaves the
    /// default in place, because a window whose keys stopped working is a
    /// worse answer to a typo than a window that says so and carries on
    pub fn from(prefix: Option<&str>, binds: &HashMap<String, String>) -> (Keys, Vec<String>) {
        let mut errs = Vec::new();
        let prefix = match prefix.map(str::trim).filter(|s| !s.is_empty()) {
            None => Trigger::parse(DEFAULT_PREFIX).expect("the built-in prefix parses"),
            Some(text) => match Trigger::parse(text) {
                Some(t) => t,
                None => {
                    errs.push(crate::i18n::tp("err.keys.unreadable", &[("key", text)]));
                    Trigger::parse(DEFAULT_PREFIX).expect("the built-in prefix parses")
                }
            },
        };
        let mut typed: HashMap<char, char> = HashMap::new();
        for a in ACTIONS {
            typed.insert(a.key, a.key);
            for c in a.also {
                typed.insert(*c, a.key);
            }
        }
        let mut direct: Vec<(Trigger, char)> = Vec::new();

        for (name, want) in binds {
            let Some(action) = ACTIONS.iter().find(|a| a.name == name) else {
                errs.push(crate::i18n::tp("err.keys.no_action", &[("name", name)]));
                continue;
            };
            let want = want.trim();
            // Off is a real answer. Someone who never splits panes should be
            // able to have that character back for their shell
            let off = want.is_empty() || want.eq_ignore_ascii_case("off");
            // The defaults this action came with stop meaning it the moment it
            // is moved, or the old key would go on working and the person
            // would never see their change take effect
            typed.retain(|_, v| *v != action.key);
            if off {
                continue;
            }
            let Some(t) = Trigger::parse(want) else {
                errs.push(crate::i18n::tp("err.keys.unreadable", &[("key", want)]));
                typed.insert(action.key, action.key);
                continue;
            };
            match (t.code, t.mods.is_empty()) {
                // A bare character means "after the prefix" -- that is what a
                // prefix is for, and a bare letter on its own would be typed
                // into whatever program is in the tab instead
                (KeyCode::Char(c), true) => {
                    if let Some(taken) = typed.get(&c) {
                        errs.push(clash(&t.show(), taken));
                        continue;
                    }
                    typed.insert(c, action.key);
                }
                _ => {
                    if let Some((_, taken)) = direct.iter().find(|(o, _)| *o == t) {
                        errs.push(clash(&t.show(), taken));
                        continue;
                    }
                    if t.matches(&KeyEvent::new(prefix.code, prefix.mods)) {
                        errs.push(crate::i18n::tp("err.keys.is_prefix", &[("key", &t.show())]));
                        continue;
                    }
                    direct.push((t, action.key));
                }
            }
        }
        (Keys { prefix, typed, direct }, errs)
    }

    /// Read straight out of the settings, and become the keys in force.
    ///
    /// Working out a key table and *adopting* it are two things: the first is
    /// asked of it in tests, several at once, and only the second may touch
    /// the one value the whole process shares
    pub fn load(cfg: Option<&crate::config::Config>) -> (Keys, Vec<String>) {
        let (keys, errs) = match cfg.map(|c| &c.keys) {
            None => (Keys::default(), Vec::new()),
            Some(k) => Keys::from(k.prefix.as_deref(), &k.binds),
        };
        if let Ok(mut g) = IN_FORCE.lock() {
            *g = Some(keys.prefix);
        }
        (keys, errs)
    }

    pub fn is_prefix(&self, ev: &KeyEvent) -> bool {
        self.prefix.matches(ev)
    }

    pub fn prefix_shown(&self) -> String {
        self.prefix.show()
    }

    /// What a press after the prefix really means.
    ///
    /// Digits are passed through untouched: they are not one action but the
    /// tabs themselves, and there is no more to say about "3" than that it is
    /// the third one
    pub fn after_prefix(&self, code: KeyCode) -> Option<KeyCode> {
        match code {
            KeyCode::Char(c) if c.is_ascii_digit() => Some(code),
            KeyCode::Char(c) => self.typed.get(&c).map(|k| KeyCode::Char(*k)),
            other => Some(other),
        }
    }

    /// What a press with no prefix means, for the combos that need none.
    pub fn direct(&self, ev: &KeyEvent) -> Option<KeyCode> {
        self.direct
            .iter()
            .find(|(t, _)| t.matches(ev))
            .map(|(_, c)| KeyCode::Char(*c))
    }

    /// The help screen: every action, with the keys that actually reach it.
    ///
    /// Built from the same table the window dispatches on, so it cannot drift
    /// from what the keys really do -- which is the whole reason the help text
    /// stopped carrying "Ctrl+B" spelled out inside it
    pub fn help_rows(&self) -> Vec<(String, &'static str)> {
        ACTIONS
            .iter()
            .filter_map(|a| {
                let mut ways: Vec<String> = self
                    .direct
                    .iter()
                    .filter(|(_, c)| *c == a.key)
                    .map(|(t, _)| t.show())
                    .collect();
                let mut after: Vec<char> =
                    self.typed.iter().filter(|(_, v)| **v == a.key).map(|(k, _)| *k).collect();
                after.sort_unstable();
                let prefix = self.prefix.show();
                ways.extend(after.into_iter().map(|c| format!("{prefix} {c}")));
                // An action nobody can reach is not worth a line
                (!ways.is_empty()).then(|| (ways.join(" / "), a.desc))
            })
            .collect()
    }
}

/// The character an action is dispatched on, by name. Used to run an action
/// the palette picked without the palette needing to know the keys.
pub fn char_for(name: &str) -> Option<char> {
    ACTIONS.iter().find(|a| a.name == name).map(|a| a.key)
}

/// Every action, as (name, description key), for a launcher that lists them.
/// The palette runs one by name; the description is what a person reads
pub fn listing() -> Vec<(&'static str, &'static str)> {
    ACTIONS.iter().map(|a| (a.name, a.desc)).collect()
}

fn clash(key: &str, taken_by: &char) -> String {
    let name = ACTIONS
        .iter()
        .find(|a| a.key == *taken_by)
        .map(|a| a.name)
        .unwrap_or("?");
    crate::i18n::tp("err.keys.taken", &[("key", key), ("name", name)])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(c: char, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), mods)
    }

    #[test]
    fn every_action_is_reachable_and_says_it_only_once() {
        let mut seen: Vec<char> = Vec::new();
        for a in ACTIONS {
            for c in std::iter::once(&a.key).chain(a.also) {
                assert!(!seen.contains(c), "{c} が二重に割り当てられている ({})", a.name);
                seen.push(*c);
            }
            assert!(!a.key.is_ascii_digit(), "{}: 数字はタブ選択のもの", a.name);
            assert!(a.name.chars().all(|c| c.is_ascii_lowercase() || c == '_'));
        }
        // Every line the help screen shows has to exist in the dictionary
        let en: serde_json::Value =
            serde_json::from_str(include_str!("../lang/en.json")).unwrap();
        for a in ACTIONS {
            assert!(en.get(a.desc).is_some(), "{} の説明が辞書に無い", a.desc);
        }
    }

    #[test]
    fn keys_are_written_the_way_people_write_them() {
        assert_eq!(Trigger::parse("ctrl+b"), Trigger::parse("Control+B"));
        assert_eq!(Trigger::parse("ctrl+b").unwrap().show(), "Ctrl+B");
        assert_eq!(Trigger::parse("alt+f4").unwrap().show(), "Alt+F4");
        assert_eq!(Trigger::parse("%").unwrap().show(), "%");
        assert!(Trigger::parse("ctrl+shift+pgup").is_some());
        assert!(Trigger::parse("").is_none());
        assert!(Trigger::parse("ctrl+nonsense").is_none());
    }

    #[test]
    fn the_shift_already_in_a_character_is_not_asked_for_twice() {
        // A terminal reports `%`, not shift+5. Comparing shift as well would
        // mean a bound `%` never fired
        let t = Trigger::parse("%").unwrap();
        assert!(t.matches(&press('%', KeyModifiers::SHIFT)));
        assert!(t.matches(&press('%', KeyModifiers::NONE)));
        assert!(!t.matches(&press('%', KeyModifiers::CONTROL)));
    }

    #[test]
    fn a_moved_action_stops_answering_to_the_key_it_left() {
        let binds = HashMap::from([("split_beside".to_string(), "ctrl+shift+d".to_string())]);
        let (keys, errs) = Keys::from(None, &binds);
        assert!(errs.is_empty(), "{errs:?}");
        // Its old keys no longer mean it...
        assert_eq!(keys.after_prefix(KeyCode::Char('%')), None);
        assert_eq!(keys.after_prefix(KeyCode::Char('|')), None);
        // ...and the new one does, with no prefix in front of it
        assert_eq!(
            keys.direct(&press('d', KeyModifiers::CONTROL | KeyModifiers::SHIFT)),
            Some(KeyCode::Char('%'))
        );
        // Everything else is where it was
        assert_eq!(keys.after_prefix(KeyCode::Char('q')), Some(KeyCode::Char('q')));
    }

    #[test]
    fn a_key_can_be_given_back() {
        let binds = HashMap::from([("literal_prefix".to_string(), "off".to_string())]);
        let (keys, errs) = Keys::from(None, &binds);
        assert!(errs.is_empty());
        assert_eq!(keys.after_prefix(KeyCode::Char('b')), None, "返した鍵は効かない");
        assert!(
            !keys.help_rows().iter().any(|(_, d)| *d == "keys.literal_prefix"),
            "届かない操作をヘルプに載せない"
        );
    }

    #[test]
    fn two_actions_cannot_share_one_key() {
        let binds = HashMap::from([("help".to_string(), "q".to_string())]);
        let (keys, errs) = Keys::from(None, &binds);
        assert_eq!(errs.len(), 1, "取り合いを黙って通さない: {errs:?}");
        // The one that was already there keeps it
        assert_eq!(keys.after_prefix(KeyCode::Char('q')), Some(KeyCode::Char('q')));
    }

    #[test]
    fn a_button_presses_the_prefix_the_person_actually_has() {
        // The page's buttons press keys. Moving the prefix and leaving them
        // pressing the old one is how every button in the app stops working
        // without a single error anywhere.
        //
        // Read before anything is adopted, so this says what a fresh process
        // does rather than what some other test left behind
        assert_eq!(prefix_now(), Trigger::parse("ctrl+b").unwrap());
    }

    #[test]
    fn the_prefix_itself_can_be_moved_and_the_help_follows() {
        let (keys, errs) = Keys::from(Some("ctrl+a"), &HashMap::new());
        assert!(errs.is_empty());
        assert!(keys.is_prefix(&press('a', KeyModifiers::CONTROL)));
        assert!(!keys.is_prefix(&press('b', KeyModifiers::CONTROL)));
        assert_eq!(keys.prefix_shown(), "Ctrl+A");
        let rows = keys.help_rows();
        assert!(
            rows.iter().any(|(k, _)| k.contains("Ctrl+A q")),
            "ヘルプが実際の鍵を出していない: {rows:?}"
        );
        assert!(!rows.iter().any(|(k, _)| k.contains("Ctrl+B")), "古い鍵が残っている");
    }

    #[test]
    fn a_typo_leaves_the_key_where_it_was() {
        let binds = HashMap::from([("quit".to_string(), "ctrl+".to_string())]);
        let (keys, errs) = Keys::from(None, &binds);
        assert_eq!(errs.len(), 1);
        assert_eq!(
            keys.after_prefix(KeyCode::Char('q')),
            Some(KeyCode::Char('q')),
            "打ち間違いで終了できなくなってはいけない"
        );
    }

    #[test]
    fn an_action_nobody_has_heard_of_is_said_out_loud() {
        let binds = HashMap::from([("teleport".to_string(), "ctrl+t".to_string())]);
        let (_, errs) = Keys::from(None, &binds);
        assert_eq!(errs.len(), 1, "知らない名前を黙って捨てない");
    }

    #[test]
    fn the_prefix_cannot_be_taken_by_something_else() {
        let binds = HashMap::from([("quit".to_string(), "ctrl+b".to_string())]);
        let (keys, errs) = Keys::from(None, &binds);
        assert_eq!(errs.len(), 1, "接頭鍵を奪わせない: {errs:?}");
        assert!(keys.is_prefix(&press('b', KeyModifiers::CONTROL)));
    }

    #[test]
    fn digits_stay_the_tabs_themselves() {
        let (keys, _) = Keys::from(None, &HashMap::new());
        for c in "0123456789".chars() {
            assert_eq!(keys.after_prefix(KeyCode::Char(c)), Some(KeyCode::Char(c)));
        }
    }
}

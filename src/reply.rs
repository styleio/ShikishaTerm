//! Answering a notification without opening the board.
//!
//! A message arrives on a phone saying a tab has finished, and the reply to it
//! is usually four words. Getting those four words back used to mean opening
//! the board, waiting for it to connect, finding the tab, and typing into the
//! composer -- four steps to say "yes, go ahead".
//!
//! So a notification can carry a link to one small page: the tab's name, what
//! it said, and a box to answer in. Nothing else, because nothing else is
//! being decided.
//!
//! **The link is not the board's key.** Putting the access token in a message
//! would hand whoever reads that channel the whole program -- every tab, the
//! settings, the files. What travels instead is a ticket: a short random
//! string that this table maps to one tab, and that can do exactly one thing,
//! which is to say something to that tab. Anything else it might be asked for
//! is not written here to refuse, it simply has nowhere to go.
//!
//! Tickets die three ways: their own expiry, the board's "disconnect" (the
//! same gesture that cuts every phone), and the program ending. There is no
//! file: a ticket that outlived the run it belongs to would be a key left
//! under a mat.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long a ticket stays good.
///
/// Long enough to answer after a night's sleep, short enough that a link in a
/// chat history is not a permanent door. A person who wants to say more after
/// that gets a new notification, which carries a new ticket.
const LIFETIME: Duration = Duration::from_secs(24 * 60 * 60);

/// Ticket ids are this long. Base62, so 12 characters is about 71 bits --
/// unguessable at any rate a network allows, and short enough that the whole
/// link fits on one line of a phone's screen.
const ID_LEN: usize = 12;

/// The most tickets kept at once. One notification mints one, so a talkative
/// week of automation is still nowhere near this; the cap exists so a hook in
/// a loop cannot grow the table without end.
const MAX_TICKETS: usize = 500;

#[derive(Clone)]
pub struct Ticket {
    /// Which tab this answers. The tab's own id when it has one, so a restart
    /// still lands in the same conversation; its number otherwise
    pub tab_id: Option<String>,
    pub tab_index: usize,
    /// The name shown on the page, so it reads like the notification did
    pub tab_name: String,
    /// What was said, shown above the box. The whole of it -- the chat message
    /// carries an opening line, this page has room for the rest
    pub said: String,
    /// Where to report back that the reply went through. Empty means the
    /// primary destination
    pub dest: String,
    born: Instant,
}

impl Ticket {
    /// Written where a notification is built, which is the only place that
    /// knows all five answers at once.
    pub fn new(
        tab_id: Option<String>,
        tab_index: usize,
        tab_name: String,
        said: String,
        dest: String,
    ) -> Self {
        Self {
            tab_id,
            tab_index,
            tab_name,
            said,
            dest,
            born: Instant::now(),
        }
    }

    fn alive(&self) -> bool {
        self.born.elapsed() < LIFETIME
    }
}

#[derive(Default)]
pub struct Book {
    tickets: Mutex<HashMap<String, Ticket>>,
}

impl Book {
    pub fn new() -> Self {
        Self::default()
    }

    /// Write a ticket and hand back its id.
    pub fn mint(&self, t: Ticket) -> String {
        let id = new_id();
        let mut all = self.tickets.lock().unwrap_or_else(|e| e.into_inner());
        // Expired ones go first, so the cap is never reached by ghosts
        all.retain(|_, t| t.alive());
        if all.len() >= MAX_TICKETS {
            // Drop the oldest rather than refuse the newest: the one being
            // written now is the one somebody is about to be shown
            if let Some(oldest) = all
                .iter()
                .min_by_key(|(_, t)| t.born)
                .map(|(k, _)| k.clone())
            {
                all.remove(&oldest);
            }
        }
        all.insert(id.clone(), t);
        id
    }

    /// The ticket behind an id, if it is one of ours and still good.
    pub fn get(&self, id: &str) -> Option<Ticket> {
        let mut all = self.tickets.lock().unwrap_or_else(|e| e.into_inner());
        match all.get(id) {
            Some(t) if t.alive() => Some(t.clone()),
            // Reaching a dead one is what expiry means; take it out on the way
            Some(_) => {
                all.remove(id);
                None
            }
            None => None,
        }
    }

    /// Tear up every ticket. The board's "disconnect" ends the phones and this
    /// in the same breath: a person who has decided nobody is holding the
    /// terminal means the links in the chat too.
    pub fn cut(&self) {
        self.tickets
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.tickets.lock().unwrap().len()
    }
}

/// A ticket id: base62, from the same source of randomness the access token
/// comes from.
///
/// Built out of the hex the rest of the program already uses rather than
/// drawing bytes another way -- two hex characters carry a byte, and folding
/// that byte into an alphabet of 62 loses a fraction of a bit that 71 bits can
/// afford.
fn new_id() -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let hex = crate::random_hex(ID_LEN);
    hex.as_bytes()
        .chunks(2)
        .take(ID_LEN)
        .map(|pair| {
            let n = u8::from_str_radix(std::str::from_utf8(pair).unwrap_or("0"), 16).unwrap_or(0);
            ALPHABET[n as usize % ALPHABET.len()] as char
        })
        .collect()
}

/// The whole link, for a notification to carry.
pub fn link(origin: &str, id: &str) -> String {
    format!("{}/r/{id}", origin.trim_end_matches('/'))
}

/// Choices a program is offering, read off the screen it is showing.
///
/// A confirmation is the commonest thing a phone is asked to answer, and
/// typing "2" into a box to say "no" is a poor way to spend a tap. The shapes
/// here are the ones the CLIs actually draw -- a number, a separator, a label
/// -- and nothing else is guessed at: an unrecognised screen offers no
/// buttons, and the box below it is still there.
pub fn choices_of(screen: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for line in screen.lines() {
        // Drop the pointer a menu draws beside the highlighted row
        let t = line.trim().trim_start_matches(['>', '\u{276f}', '*', '-', ' ']).trim();
        let mut chars = t.chars();
        let Some(key) = chars.next().filter(|c| c.is_ascii_digit() && *c != '0') else {
            continue;
        };
        let rest = chars.as_str();
        let Some(label) = rest
            .strip_prefix('.')
            .or_else(|| rest.strip_prefix(')'))
            .or_else(|| rest.strip_prefix(':'))
        else {
            continue;
        };
        // A space after the separator is what tells a menu from a decimal:
        // "1. Yes" is a choice, "3.14 is pi" is a number in a sentence
        let Some(label) = label.strip_prefix([' ', '\t']) else {
            continue;
        };
        let label = label.trim();
        // A number with nothing after it is a line number, not a choice; a
        // whole paragraph after it is prose that happens to start with one
        if label.is_empty() || label.chars().count() > 60 {
            continue;
        }
        let key = key.to_string();
        if out.iter().any(|(k, _)| *k == key) {
            continue;
        }
        out.push((key, label.to_string()));
    }
    // Two or more, or it is not a choice
    if out.len() < 2 {
        return Vec::new();
    }
    out.truncate(9);
    out
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn js(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

/// The whole page: one tab, what it said, and a box to answer in.
///
/// Deliberately not the board. The board is for looking at everything; this is
/// for answering one question, and every control that is not that would be a
/// way to press the wrong thing on a phone, one-handed, on a train.
pub fn page(id: &str, t: &Ticket) -> String {
    let buttons: String = choices_of(&t.said)
        .iter()
        .map(|(k, label)| {
            format!(
                "<button class=\"choice\" data-say=\"{}\">{}. {}</button>",
                esc(k),
                esc(k),
                esc(label)
            )
        })
        .collect();
    format!(
        r##"<!doctype html><html lang="{lang}"><head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1,viewport-fit=cover">
<meta name="referrer" content="no-referrer">
<title>{name}</title>
<style>
:root {{ color-scheme: dark; --bg:#11131a; --fg:#e8eaf0; --dim:#9aa3b2;
        --line:#262a36; --accent:#3aa0ff; --card:#171a23; }}
* {{ box-sizing:border-box; }}
body {{ margin:0 auto; background:var(--bg); color:var(--fg); padding:16px;
        max-width:720px; font:16px/1.6 system-ui,"Segoe UI",
        "Hiragino Kaku Gothic ProN","Noto Sans JP",sans-serif; }}
h1 {{ font-size:13px; margin:0 0 6px; letter-spacing:.12em; color:var(--dim); }}
.tab {{ font-size:20px; font-weight:600; margin:0 0 12px; }}
.said {{ background:var(--card); border:1px solid var(--line); border-radius:10px;
         padding:12px 14px; white-space:pre-wrap; word-break:break-word;
         max-height:44vh; overflow:auto; margin:0 0 16px; }}
.choices {{ display:flex; flex-wrap:wrap; gap:8px; margin:0 0 14px; }}
button {{ font:inherit; border-radius:10px; border:1px solid var(--line);
          background:var(--card); color:var(--fg); padding:10px 14px; cursor:pointer; }}
button.send {{ background:var(--accent); border-color:var(--accent); color:#04121f;
               font-weight:600; width:100%; padding:14px; margin-top:10px; }}
button:disabled {{ opacity:.5; cursor:default; }}
textarea {{ width:100%; min-height:110px; font:inherit; padding:12px;
            border-radius:10px; border:1px solid var(--line);
            background:var(--card); color:var(--fg); resize:vertical; }}
.you {{ font-size:13px; color:var(--dim); margin:0 0 6px; }}
.done {{ border-left:3px solid var(--accent); padding-left:12px; white-space:pre-wrap; }}
.note {{ color:var(--dim); font-size:13px; margin-top:18px; }}
.err {{ color:#ff8f8f; }}
</style></head><body>
<h1>SHIKISHA-TERM</h1>
<p class="tab">{name}</p>
<div class="said">{said}</div>
<div class="choices">{buttons}</div>
<div id="form">
  <p class="you">{you}</p>
  <textarea id="text" autofocus></textarea>
  <button class="send" id="send">{send}</button>
</div>
<p class="note" id="note"></p>
<script>
const ID = {id};
const T = {{ sent: {t_sent}, failed: {t_failed} }};
const form = document.getElementById("form");
const note = document.getElementById("note");
async function say(text) {{
  if (!text.trim()) return;
  document.getElementById("send").disabled = true;
  let ok = false;
  try {{
    const r = await fetch("/r/" + ID + "/say", {{
      method: "POST",
      headers: {{ "Content-Type": "application/json" }},
      body: JSON.stringify({{ text: text }}),
    }});
    ok = r.ok && (await r.json()).ok;
  }} catch (e) {{ ok = false; }}
  if (!ok) {{
    note.textContent = T.failed;
    note.className = "note err";
    document.getElementById("send").disabled = false;
    return;
  }}
  // The page becomes the receipt: what was sent, in the words that were sent
  form.innerHTML = "";
  const p = document.createElement("p");
  p.className = "you";
  p.textContent = T.sent;
  const d = document.createElement("div");
  d.className = "done";
  d.textContent = text;
  form.append(p, d);
  document.querySelectorAll("button.choice").forEach(b => b.remove());
}}
document.getElementById("send").addEventListener("click",
  () => say(document.getElementById("text").value));
document.querySelectorAll("button.choice").forEach(b =>
  b.addEventListener("click", () => say(b.dataset.say)));
// Ctrl/Cmd+Enter sends from a hardware keyboard. A bare Enter stays a
// newline, because on a phone that key is how you write a second line
document.getElementById("text").addEventListener("keydown", e => {{
  if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) say(e.target.value);
}});
</script>
</body></html>"##,
        lang = esc(&crate::i18n::lang()),
        name = esc(&t.tab_name),
        said = esc(&t.said),
        buttons = buttons,
        you = esc(&crate::i18n::t("reply.you")),
        send = esc(&crate::i18n::t("reply.send")),
        id = js(id),
        t_sent = js(&crate::i18n::t("reply.sent")),
        t_failed = js(&crate::i18n::t("reply.failed")),
    )
}

/// The page shown when a ticket is not one of ours any more.
pub fn gone_page() -> String {
    format!(
        r#"<!doctype html><html lang="{lang}"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>SHIKISHA-TERM</title>
<style>body {{ margin:0; background:#11131a; color:#e8eaf0; padding:24px;
  font:16px/1.7 system-ui,"Segoe UI","Noto Sans JP",sans-serif; }}
p {{ max-width:32em; }}</style></head><body>
<p>{msg}</p></body></html>"#,
        lang = esc(&crate::i18n::lang()),
        msg = esc(&crate::i18n::t("reply.gone")),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ticket(name: &str) -> Ticket {
        Ticket {
            tab_id: Some("coder".into()),
            tab_index: 1,
            tab_name: name.into(),
            said: "終わりました".into(),
            dest: String::new(),
            born: Instant::now(),
        }
    }

    #[test]
    fn a_ticket_comes_back_by_its_id_and_nothing_else_does() {
        let book = Book::new();
        let id = book.mint(ticket("レビュワー"));
        assert_eq!(id.chars().count(), ID_LEN);
        assert!(id.chars().all(|c| c.is_ascii_alphanumeric()), "URLに置ける文字だけ");
        assert_eq!(book.get(&id).unwrap().tab_name, "レビュワー");
        // A guess is not a ticket
        assert!(book.get("ZZZZZZZZZZZZ").is_none());
        assert!(book.get("").is_none());
        assert!(book.get(&id.to_lowercase()).is_none() || id == id.to_lowercase());
    }

    #[test]
    fn two_tickets_are_not_the_same_ticket() {
        let book = Book::new();
        let a = book.mint(ticket("a"));
        let b = book.mint(ticket("b"));
        assert_ne!(a, b);
        assert_eq!(book.get(&a).unwrap().tab_name, "a");
        assert_eq!(book.get(&b).unwrap().tab_name, "b");
    }

    /// The board's disconnect is one gesture with one meaning: nobody is
    /// holding this terminal now. A link left in a chat is somebody holding it.
    #[test]
    fn disconnecting_tears_up_every_ticket() {
        let book = Book::new();
        let id = book.mint(ticket("x"));
        book.cut();
        assert!(book.get(&id).is_none());
        assert_eq!(book.len(), 0);
    }

    #[test]
    fn an_expired_ticket_opens_nothing_and_does_not_linger() {
        let book = Book::new();
        let mut old = ticket("x");
        old.born = Instant::now() - (LIFETIME + Duration::from_secs(1));
        let id = book.mint(old);
        assert!(book.get(&id).is_none(), "期限切れは通らない");
        assert_eq!(book.len(), 0, "引いた時点で捨てる");
    }

    /// A hook in a loop must not be able to grow this without end.
    #[test]
    fn the_table_has_a_ceiling() {
        let book = Book::new();
        let first = book.mint(ticket("first"));
        for i in 0..MAX_TICKETS + 10 {
            book.mint(ticket(&format!("t{i}")));
        }
        assert!(book.len() <= MAX_TICKETS, "上限を超えない: {}", book.len());
        assert!(book.get(&first).is_none(), "古いものから捨てる");
    }

    /// The commonest thing a phone is asked is a confirmation, and answering
    /// it should be a tap. What may not happen is buttons appearing on prose
    /// that merely starts with a number.
    #[test]
    fn a_confirmation_becomes_buttons_and_prose_does_not() {
        let asked = "Do you want to make this edit?\n\u{276f} 1. Yes\n  2. Yes, and don't ask again\n  3. No, tell Claude what to do differently";
        let c = choices_of(asked);
        assert_eq!(c.len(), 3);
        assert_eq!(c[0], ("1".into(), "Yes".into()));
        assert_eq!(c[2].0, "3");
        assert!(c[2].1.starts_with("No,"));

        // Japanese, and the other separators the CLIs use
        let ja = "続けますか\n1) はい\n2) いいえ";
        assert_eq!(choices_of(ja).len(), 2);
        assert_eq!(choices_of("1: a\n2: b").len(), 2);

        // One is not a choice, and neither is a paragraph that opens with a
        // number, nor a line of numbers with nothing after them
        assert!(choices_of("1. Yes").is_empty(), "選択肢が1つなら選ばせない");
        assert!(choices_of("").is_empty());
        assert!(choices_of("3.14 is pi\n2. and this is a real option").is_empty());
        let prose = format!("1. {}\n2. {}", "x".repeat(80), "y".repeat(80));
        assert!(choices_of(&prose).is_empty(), "長すぎるものは選択肢ではない");
        // The same number twice is a screen we do not understand; take the first
        assert_eq!(choices_of("1. a\n1. b\n2. c").len(), 2);
    }

    /// The page carries what the person needs and nothing that could hurt
    /// them: no token, and no way to type HTML into it from a tab's output.
    #[test]
    fn the_page_shows_the_answer_and_leaks_nothing() {
        crate::i18n::init(Some("en"), &[std::path::PathBuf::from("lang")]);
        let mut t = ticket("レビュワー");
        t.said = "<script>alert(1)</script> & \"done\"".into();
        let html = page("K3fQ92mZxAbC", &t);
        assert!(html.contains("レビュワー"), "タブ名が出る");
        assert!(!html.contains("<script>alert"), "出力はHTMLとして解釈させない");
        assert!(html.contains("&lt;script&gt;"), "文字としては見える");
        assert!(html.contains("K3fQ92mZxAbC"), "自分の切符でPOSTできる");
        assert!(!html.contains("?t="), "盤面のトークンは載らない");

        // A confirmation gets its buttons; a plain answer does not
        let mut asking = ticket("coder");
        asking.said = "Proceed?\n 1. Yes\n 2. No".into();
        let html = page("x", &asking);
        assert_eq!(html.matches("class=\"choice\"").count(), 2);
    }

    #[test]
    fn the_link_is_the_origin_and_the_ticket() {
        assert_eq!(link("http://100.64.1.2:8787", "AbC"), "http://100.64.1.2:8787/r/AbC");
        // A trailing slash on the origin does not double up
        assert_eq!(link("http://x:1/", "AbC"), "http://x:1/r/AbC");
    }
}

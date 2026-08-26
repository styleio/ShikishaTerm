//! The one toast every surface speaks through.
//!
//! Short messages appear on three screens — the window/phone shell, the
//! settings screen and the transcript view — and each of them used to carry its
//! own message bar with its own timing. That drifted: the settings toast faded
//! after a couple of seconds, the shell's attach toast after three, and the
//! shell's message line (fed by the app's `flash`) faded never — it waited for
//! a keystroke, so a notice shown at startup could still be sitting there an
//! hour later. None of them could be dismissed by hand.
//!
//! So the toast lives here once. It fades on its own after a few seconds — a
//! little longer for a warning, longer again for a long message, because a
//! notice you can't finish reading is the same as no notice — and a click or
//! tap takes it away immediately.
//!
//! Copying is a separate gesture on a button of its own, and deliberately so.
//! A message worth pasting somewhere (an error, a saved path) is common enough
//! to deserve one press, but making the *dismissal* copy would overwrite the
//! clipboard of anyone who only wanted the thing out of the way — losing what
//! they were carrying to paste. Getting rid of a message must cost nothing.
//!
//! A page drops [`CSS`] into its stylesheet, [`HTML`] into its body and [`JS`]
//! into its script, then calls `toast(text, warn)`. Two things stay the page's
//! own business, because they genuinely differ per surface, and each is a hook
//! the page may define:
//!
//! - `toastBottom()` — how high off this screen's floor the toast sits (the
//!   window has a composer bar it must not cover; a web page just has a floor)
//! - `toastText(text, warn)` — wording. The settings screen marks its results
//!   with ✓ / ⚠; the window's messages arrive already written by the app.
//!
//! Deliberately free of `__` and `{{`: every page these strings land in is
//! checked for leftover placeholders after rendering, and the shell page in
//! particular is never run through the dictionary — so wording comes from `T`,
//! the dictionary each page already carries in its script.

/// Look and placement. Colours come from the app's scheme, so the toast is the
/// same object on every screen; `--toast-pos` / `--toast-x` / `--toast-bottom`
/// / `--toast-max` / `--toast-z` let a page seat it in its own layout. The
/// window uses them to sit it over the focused pane rather than the whole
/// window: centred on the window, a split with a page in one half cut the
/// message in two at that pane's edge.
pub const CSS: &str = r#"
 #toast { position:var(--toast-pos, fixed); left:var(--toast-x, 50%); bottom:var(--toast-bottom, 28px);
   transform:translateX(-50%) translateY(16px);
   display:flex; align-items:center; gap:10px;
   max-width:var(--toast-max, min(86%, 560px)); padding:10px 12px 10px 18px; border-radius:9px;
   background:var(--accent); color:#04121c; font-weight:600; font-size:13.5px;
   line-height:1.5; text-align:left;
   box-shadow:0 10px 30px rgba(0,0,0,.5); cursor:pointer;
   opacity:0; z-index:var(--toast-z, 50);
   transition:opacity .18s ease, transform .18s ease;
   /* Catches clicks only while it is up. An invisible toast that still
      swallowed taps would break whatever sits underneath it */
   pointer-events:none; }
 #toast.show { opacity:1; transform:translateX(-50%) translateY(0); pointer-events:auto; }
 #toast.warn { background:var(--danger); color:#fff; }
 /* Long words (a path, a URL, a stack line) wrap rather than widen the toast
    past the screen. Up to six lines of an error, then it gives up scrolling
    and lets the message be copied out instead */
 #toastmsg { flex:1 1 auto; min-width:0; overflow-wrap:anywhere;
   max-height:9em; overflow:hidden; }
 #toastcopy { flex:none; align-self:flex-start; border:0; border-radius:6px;
   padding:3px 7px; font-family:inherit; font-size:13px; line-height:1.3;
   cursor:pointer; color:inherit; background:rgba(0,0,0,.16); }
 #toastcopy:hover { background:rgba(0,0,0,.3); }
"#;

/// The element itself. One per page, near the end of the body.
///
/// The live region is the message alone: a screen reader announcing the copy
/// button's label along with every notice would be reading out furniture.
pub const HTML: &str = r#"<div id="toast"><span id="toastmsg" role="status" aria-live="polite"></span><button id="toastcopy" type="button"></button></div>"#;

/// Showing, timing, copying and dismissing. `toast(text, warn)` is the whole
/// API; `hideToast()` is there for a page that knows a message has stopped
/// applying.
pub const JS: &str = r#"
let toastTimer = null;
function toast(text, warn) {
  const t = document.getElementById("toast"), m = document.getElementById("toastmsg");
  if (!t || !m) return;
  const s = String(text === null || text === undefined ? "" : text);
  // Nothing to say is not a message; it's the absence of one
  if (!s.trim()) { hideToast(); return; }
  m.textContent = (typeof toastText === "function") ? toastText(s, !!warn) : s;
  t.classList.toggle("warn", !!warn);
  const b = document.getElementById("toastcopy");
  if (b) {
    b.textContent = "📋";
    b.title = b.ariaLabel = (typeof T === "object" && T && T["toast.copy"]) || "Copy";
  }
  if (typeof toastBottom === "function") {
    try { t.style.bottom = toastBottom(); } catch (e) {}
  }
  t.classList.add("show");
  clearTimeout(toastTimer);
  toastTimer = setTimeout(hideToast, toastMs(m.textContent, warn));
}
// Roughly how long it takes to read: a floor so a single word doesn't blink
// past, a ceiling so nothing overstays its welcome, and a warning gets long
// enough to be acted on
function toastMs(text, warn) {
  return Math.min(9000, Math.max(warn ? 6000 : 3000, 500 + text.length * 90));
}
function hideToast() {
  clearTimeout(toastTimer);
  toastTimer = null;
  const t = document.getElementById("toast");
  if (t) t.classList.remove("show");
}
// Take the message with you. Only from the button: a click meant to get a
// notice out of the way must never overwrite what you were about to paste
function copyToast() {
  const m = document.getElementById("toastmsg"), b = document.getElementById("toastcopy");
  copyText(m ? m.textContent : "").then(() => {
    // A toast confirming a toast would be absurd, so the button says it took
    // and then the whole thing leaves, which is what the press asked for
    if (b) b.textContent = "✓";
    clearTimeout(toastTimer);
    toastTimer = setTimeout(hideToast, 550);
  });
}
// navigator.clipboard only exists in a secure context, and a phone reaching
// this over a plain address on the home network is not one — so the old way is
// a real fallback here, not an afterthought
function copyText(text) {
  if (navigator.clipboard && navigator.clipboard.writeText) {
    return navigator.clipboard.writeText(text).catch(() => legacyCopy(text));
  }
  return Promise.resolve(legacyCopy(text));
}
function legacyCopy(text) {
  // Copying moves the selection, and the selection is the keyboard: hand focus
  // back to whatever had it, or the next keystroke goes nowhere
  const was = document.activeElement;
  const a = document.createElement("textarea");
  a.value = text;
  a.setAttribute("readonly", "");
  a.style.cssText = "position:fixed;top:-1000px;opacity:0";
  document.body.append(a);
  a.select();
  try { document.execCommand("copy"); } catch (e) {}
  a.remove();
  if (was && was.focus) { try { was.focus(); } catch (e) {} }
}
// A message you have already read shouldn't have to be waited out. Taken in the
// capture phase and stopped there: the toast is an overlay, so a tap aimed at
// it must not also land on the pane, button or link underneath it
document.addEventListener("click", e => {
  const t = document.getElementById("toast");
  if (!t || !t.classList.contains("show") || !t.contains(e.target)) return;
  e.preventDefault();
  e.stopPropagation();
  const b = document.getElementById("toastcopy");
  if (b && b.contains(e.target)) copyToast(); else hideToast();
}, true);
"#;

/// Drops the shared toast into a page. Every screen that shows messages calls
/// this while it builds its HTML; a page that forgot to would come up with the
/// markers still written across it, which the render tests catch.
pub fn render(html: String) -> String {
    html.replace("{{TOAST_CSS}}", CSS)
        .replace("{{TOAST_HTML}}", HTML)
        .replace("{{TOAST_JS}}", JS)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pages these strings land in are checked for unreplaced placeholders
    /// (`{{key}}` and `__NAME__`) after rendering. Toast code that happened to
    /// contain either would fail that check from the outside, with nothing
    /// pointing back here — so it is stated here instead.
    #[test]
    fn the_shared_block_looks_like_nobodys_placeholder() {
        for (name, part) in [("CSS", CSS), ("HTML", HTML), ("JS", JS)] {
            assert!(!part.contains("{{"), "{name} にテンプレート記法が混ざっている");
            assert!(!part.contains("__"), "{name} にプレースホルダ記法が混ざっている");
        }
    }

    /// Every binding the block introduces, so a page can be checked for having
    /// its own copy of one. A duplicate `const`/`let` at the top level is a
    /// SyntaxError that kills the entire script, leaving a screen that draws
    /// but does nothing.
    #[test]
    fn the_block_declares_each_name_once() {
        for name in [
            "let toastTimer",
            "function toast(",
            "function toastMs(",
            "function hideToast(",
            "function copyToast(",
            "function copyText(",
            "function legacyCopy(",
        ] {
            assert_eq!(JS.matches(name).count(), 1, "{name} が重複または欠落している");
        }
    }

    /// The three halves have to agree on the ids, or the toast is code talking
    /// to elements that aren't there.
    #[test]
    fn the_markup_and_the_script_mean_the_same_elements() {
        for id in ["toast", "toastmsg", "toastcopy"] {
            assert!(HTML.contains(&format!(r#"id="{id}""#)), "{id} が markup に無い");
            assert!(CSS.contains(&format!("#{id}")), "{id} に見た目が無い");
            assert!(
                JS.matches(&format!(r#"getElementById("{id}")"#)).count() > 0,
                "{id} を誰も掴んでいない"
            );
        }
    }

    /// Dismissing must stay free. If a plain click ever started copying, a
    /// press meant only to clear the screen would quietly destroy whatever the
    /// user was carrying in the clipboard — so the two gestures are pinned
    /// apart here.
    #[test]
    fn only_the_button_touches_the_clipboard() {
        let handler = JS
            .split("document.addEventListener(\"click\"")
            .nth(1)
            .expect("クリックを誰も見ていない");
        assert!(
            handler.contains("if (b && b.contains(e.target)) copyToast(); else hideToast();"),
            "クリックとコピーが分かれていない"
        );
        // Twice and no more: the one call inside copyToast(), and its own
        // definition. A third would be some other path reaching the clipboard
        assert_eq!(JS.matches("copyText(").count(), 2, "コピーを呼ぶ場所が増えている");
    }

    /// Wording comes from the dictionary each page already carries, so a key
    /// that doesn't exist there would leave the button silently unlabelled.
    #[test]
    fn every_word_the_block_asks_for_is_in_the_dictionary() {
        let en: serde_json::Value =
            serde_json::from_str(include_str!("../lang/en.json")).unwrap();
        let mut rest = JS;
        while let Some(i) = rest.find("T[\"") {
            rest = &rest[i + 3..];
            let key = &rest[..rest.find('"').expect("キーが閉じていない")];
            assert!(en.get(key).is_some(), "lang/en.json に無いキー: {key}");
        }
    }

    #[test]
    fn render_fills_every_marker() {
        let out =
            render("<style>{{TOAST_CSS}}</style>{{TOAST_HTML}}<script>{{TOAST_JS}}</script>".into());
        assert!(!out.contains("{{"), "置き換え漏れがある: {out}");
        assert!(out.contains("#toast.show"));
    }
}

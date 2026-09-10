//! Put one browser under command.
//!
//! Windows 11 ships with a Chromium engine (WebView2) built in,
//! and Microsoft keeps it updated. So we don't bundle our own. We borrow it.
//! That keeps the "no-install single exe" promise intact.
//!
//! The window runs on its own thread. The TUI's render loop and the
//! message loop both want to run on their own terms, so they must not mix.
//!
//! Use `run_return`, not `run`. `run` is `-> !` and calls
//! `process::exit` internally. Just closing the browser window would
//! take down the whole app.

use shikisha_shared::{BranchAsk, BrowserProfile, Ev, Found, Go, Input, OpReport, PaneGeom, Sel, parse_intent};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};

use anyhow::{Result, anyhow};

/// Added on top of `INIT_JS` for pages placed inside the window, and only for
/// those: the shell's own page already knows which pane was clicked.
///
/// A placed page is a native layer with its own window handle, so a press that
/// lands on it never reaches the pane underneath -- which is why a browser
/// pane could only be focused by its caption, and why the pen that summons the
/// composer never appeared over one.
///
/// What is reported is the page taking the keyboard, not a click. Automation
/// drives these pages by dispatching input at them, and dispatched input never
/// moves the window handle's focus -- so a rally clicking through a form
/// cannot pull the keyboard out from under someone typing in another pane,
/// which is exactly what reporting the click itself would have done.
const PLACED_JS: &str = r##"
(function () {
  const tell = () => window.ipc.postMessage(JSON.stringify({ kind: "touched" }));
  addEventListener("focus", tell);
  // The press that brought the focus here arrives before the focus event on
  // some paths and after it on others; asking whether we hold it is the same
  // question either way, and asking twice costs a message nobody reads
  addEventListener("pointerdown", () => { if (document.hasFocus()) tell(); }, true);

  // The pen that summons the composer, drawn HERE.
  //
  // The window cannot draw it over this page. A z-index orders things inside
  // one document; this page is a window of its own, and no element of the
  // window's page can be stacked above another window. The room for it used to
  // be taken out of the page instead -- the page was held up by the height of
  // a button, leaving a band of nothing under it. Drawn from inside, it floats
  // over the page exactly as it does over a terminal, in the same corner, and
  // costs no room at all.
  //
  // In a shadow root so the page's own CSS cannot reach it, and the other way
  // about -- same as the banner above
  let host = null;
  // The app's messages, drawn by the page for the same reason the pen is: a
  // window of its own cannot be drawn over, so a toast raised while this page
  // fills the focused pane would be hidden behind it -- or, in a split, cut in
  // half at the pane's edge. It seats itself at the bottom of THIS page, which
  // is the pane it is about.
  var toastEl = null, toastGo = 0;
  window.__shikisha_toast = function (text, warn) {
    if (!text) { if (toastEl) toastEl.style.display = "none"; return; }
    if (!toastEl) {
      toastEl = document.createElement("div");
      toastEl.id = "__shikisha_toast";
      toastEl.style.cssText =
        "position:fixed;left:50%;bottom:20px;transform:translateX(-50%);" +
        "z-index:2147483646;max-width:min(86%,560px)";
      (document.body || document.documentElement).appendChild(toastEl);
      toastEl.attachShadow({ mode: "open" });
      toastEl.shadowRoot.innerHTML =
        '<div style="padding:10px 14px;border-radius:9px;font-size:13.5px;' +
        'line-height:1.5;font-weight:600;text-align:left;overflow-wrap:anywhere;' +
        'max-height:9em;overflow:hidden;box-shadow:0 10px 30px rgba(0,0,0,.5);' +
        'font-family:system-ui,sans-serif"></div>';
    }
    var box = toastEl.shadowRoot.firstChild;
    box.style.background = warn ? "#c8382f" : "#7fd7ff";
    box.style.color = warn ? "#fff" : "#04121c";
    box.textContent = text;
    toastEl.style.display = "";
    // Long enough to read, and the same message arriving again restarts it
    clearTimeout(toastGo);
    toastGo = setTimeout(function () { if (toastEl) toastEl.style.display = "none"; },
      Math.min(12000, 3500 + text.length * 45));
  };
  window.__shikisha_pen = function (on) {
    if (!on) { if (host) host.style.display = "none"; return; }
    if (!host) {
      host = document.createElement("div");
      host.id = "__shikisha_pen";
      host.style.cssText =
        "position:fixed;right:16px;bottom:16px;z-index:2147483646";
      (document.body || document.documentElement).appendChild(host);
      host.attachShadow({ mode: "open" });
      host.shadowRoot.innerHTML =
        '<button style="width:44px;height:44px;border-radius:50%;font-size:19px;' +
        'line-height:1;display:flex;align-items:center;justify-content:center;' +
        'cursor:pointer;border:1px solid #00aaff;background:#0a0c0e;' +
        'box-shadow:0 4px 16px rgba(0,0,0,.45);opacity:.85">&#9999;&#65039;</button>';
      host.shadowRoot.querySelector("button").onclick = () =>
        window.ipc.postMessage(JSON.stringify({ kind: "compose" }));
    }
    host.style.display = "";
  };
})();
"##;

/// Added on top of the placed-page scripts for a window a page asked to open.
///
/// An adopted window has no title bar of its own — it sits in the seat its
/// opener was using — so the way out of it has to be drawn. `window.close` is
/// taken over for the same reason: a popup that finishes by closing itself
/// (which is how every sign-in popup ends) has to reach us, or the runtime
/// tears the page down from under the pane and leaves a hole.
const POPUP_JS: &str = r#"
(function () {
  const shut = () => { try { window.ipc.postMessage(JSON.stringify({kind:"popupclose"})); } catch (e) {} };
  window.close = shut;
  const draw = () => {
    if (document.getElementById("__shikisha_popbar") || !document.documentElement) return;
    const bar = document.createElement("div");
    bar.id = "__shikisha_popbar";
    bar.style.cssText = "position:fixed;top:0;left:0;right:0;height:24px;z-index:2147483647;" +
      "display:flex;align-items:center;gap:8px;padding:0 8px;box-sizing:border-box;" +
      "background:#1b1d22;color:#c9ced8;font:11px/1 system-ui,sans-serif;";
    const who = document.createElement("span");
    who.textContent = location.origin;
    who.style.cssText = "flex:1;overflow:hidden;text-overflow:ellipsis;white-space:nowrap";
    const x = document.createElement("span");
    x.textContent = "\u2715";
    x.style.cssText = "cursor:pointer;padding:0 6px;font-size:13px";
    x.addEventListener("click", shut);
    bar.append(who, x);
    document.documentElement.appendChild(bar);
  };
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", draw);
  else draw();
})();
"#;

/// Always injected into every document first.
///
/// It runs on every navigation, so the helpers automation calls into are
/// there however many times a login redirects. Nothing in here asks the
/// person anything: the bar that does is the app's own, drawn under the page
/// by the board (shell.rs), where a page cannot press it.
const INIT_JS: &str = r##"
(function () {
  if (window.__shikisha) return;
  const send = (o) => window.ipc.postMessage(JSON.stringify(o));

  // A selector is either {css:"..."} or {xpath:"..."}.
  // XPath lets us express lookups CSS can't, like "the cell just to the
  // right of the cell labeled 'Name'", so we support both
  window.__shikisha_q = function (sel) {
    if (sel && sel.xpath) {
      return document.evaluate(sel.xpath, document, null, 9, null).singleNodeValue;
    }
    return document.querySelector(sel.css);
  };

  // Distinguish "not in the DOM" from "in the DOM but off-screen".
  // Collapsing them into one failure makes it impossible to tell whether
  // to suspect the selector or the wait
  window.__shikisha_state = function (sel) {
    const el = window.__shikisha_q(sel);
    if (!el) return "not_found";
    const r = el.getBoundingClientRect();
    const on =
      r.width > 0 && r.height > 0 &&
      r.bottom > 0 && r.right > 0 &&
      r.top < innerHeight && r.left < innerWidth;
    return on ? "visible" : "off_screen";
  };

  window.__shikisha_text = function (sel) {
    const el = window.__shikisha_q(sel);
    return el ? (el.value !== undefined ? el.value : el.innerText) : null;
  };

  // Where this page actually is. Asked before a stored password is typed into
  // it: the address the page was opened at is not the address it is at now
  window.__shikisha_href = function () { return location.href; };

  // ---- Auto-wait (actionability engine) ------------------------------------
  // An action waits until its element is genuinely operable:
  //  - visible  = non-empty box AND the computed visibility chain is visible
  //               (display:contents looks through to a visible child)
  //  - stable   = the bounding rect is identical on two consecutive animation
  //               frames; frames shorter than 15ms are dropped (some engines
  //               deliver bogus extra frames)
  //  - enabled  = not natively disabled (:disabled covers fieldset
  //               inheritance) and not inside [aria-disabled="true"]
  //  - hit      = elementFromPoint at the action point, pierced through open
  //               shadow roots, climbs (via slots/hosts) back to the target
  //  - retries back off 0/20/100/100/500ms, and each retry tries the next
  //    scrollIntoView alignment (shakes off position:sticky overlays)
  const __rafTick = () => new Promise((f) => requestAnimationFrame(f));
  const __pause = (ms) => new Promise((f) => setTimeout(f, ms));
  const __BACKOFF = [0, 20, 100, 100, 500];
  function __visible(el) {
    const style = getComputedStyle(el);
    if (!style) return true;
    if (style.display === "contents") {
      for (let child = el.firstChild; child; child = child.nextSibling) {
        if (child.nodeType === 1 && __visible(child)) return true;
      }
      return false;
    }
    if (style.visibility !== "visible") return false;
    const rect = el.getBoundingClientRect();
    return rect.width > 0 && rect.height > 0;
  }
  async function __stable(el) {
    let last = null;
    let lastTime = 0;
    for (let frames = 0; frames < 12; frames++) {
      await __rafTick();
      if (!el.isConnected) return false;
      const t = performance.now();
      if (t - lastTime < 15) continue;
      lastTime = t;
      const r = el.getBoundingClientRect();
      const rect = { x: r.x, y: r.y, w: r.width, h: r.height };
      if (last) {
        return rect.x === last.x && rect.y === last.y && rect.w === last.w && rect.h === last.h;
      }
      last = rect;
    }
    return false;
  }
  function __hitOk(el, x, y) {
    let hit = document.elementFromPoint(x, y);
    while (hit && hit.shadowRoot) {
      const inner = hit.shadowRoot.elementFromPoint(x, y);
      if (!inner || inner === hit) break;
      hit = inner;
    }
    // The hit target must be the element or live inside it, judged on the
    // composed tree (slotted content climbs to its slot's host)
    let cur = hit;
    while (cur && cur !== el) {
      const root = cur.getRootNode && cur.getRootNode();
      cur = cur.assignedSlot || cur.parentElement
        || (root && root.host ? root.host : null);
    }
    return cur === el;
  }
  // Wait until the element is actionable (or the deadline runs out) and
  // report the action point. `o`: { deadline (ms from now), enabled, hit }.
  // Failures name the state that never arrived
  window.__shikisha_ready = async function (el, o) {
    const deadline = performance.now() + ((o && o.deadline) || 4000);
    const scrolls = [
      { block: "center", inline: "center" },
      { block: "end", inline: "end" },
      { block: "start", inline: "start" },
      { block: "nearest", inline: "nearest" },
    ];
    let retry = 0;
    let why = "hidden";
    while (true) {
      if (!el.isConnected) return { ok: false, why: "not_found" };
      if (!__visible(el)) {
        why = "hidden";
      } else if (o && o.enabled && el.closest(':disabled, [aria-disabled="true"]')) {
        why = "disabled";
      } else {
        el.scrollIntoView(scrolls[retry % scrolls.length]);
        if (!(await __stable(el))) {
          why = "unstable";
        } else {
          const r = el.getBoundingClientRect();
          const x = r.x + r.width / 2;
          const y = r.y + r.height / 2;
          if (o && o.hit && !__hitOk(el, x, y)) {
            why = "covered";
          } else {
            return { ok: true, x: x, y: y };
          }
        }
      }
      const wait = __BACKOFF[Math.min(retry, __BACKOFF.length - 1)];
      retry++;
      if (performance.now() + wait > deadline) return { ok: false, why: why };
      await __pause(wait);
    }
  };
  // Resolve a selector, retrying until the deadline — the half of auto-wait
  // that lets a replayed script address elements the page hasn't built yet
  window.__shikisha_resolve = async function (sel, deadline) {
    let retry = 0;
    while (true) {
      const el = window.__shikisha_q(sel);
      if (el) return el;
      const wait = __BACKOFF[Math.min(retry, __BACKOFF.length - 1)] || 100;
      retry++;
      if (performance.now() + wait > deadline) return null;
      await __pause(wait);
    }
  };

  window.__shikisha_click = async function (sel, deadline_ms) {
    const deadline = performance.now() + (deadline_ms || 4000);
    const el = await window.__shikisha_resolve(sel, deadline);
    if (!el) return "not_found";
    // Wait for actionability; when the deadline passes with the element
    // present, degrade to the pre-auto-wait behavior (honor the caller's
    // intent) instead of inventing a new failure mode. No hit check here —
    // a synthetic click() doesn't hit-test anyway
    const r = await window.__shikisha_ready(el, {
      deadline: deadline - performance.now(),
      enabled: true,
    });
    if (!r.ok) el.scrollIntoView({ block: "center" });
    el.click();
    // If we touched it, it was reachable. Keep the same vocabulary as find
    return "visible";
  };

  window.__shikisha_fill = async function (sel, value, deadline_ms) {
    const deadline = performance.now() + (deadline_ms || 4000);
    const el = await window.__shikisha_resolve(sel, deadline);
    if (!el) return "not_found";
    await window.__shikisha_ready(el, {
      deadline: deadline - performance.now(),
      enabled: true,
    });
    el.focus();
    if (el.isContentEditable) {
      el.textContent = value;
    } else {
      // Frameworks like React don't notice a direct write to value.
      // Going through the original setter before dispatching input
      // also updates the framework's own state
      const proto =
        el instanceof HTMLTextAreaElement
          ? HTMLTextAreaElement.prototype
          : el instanceof HTMLSelectElement
            ? HTMLSelectElement.prototype
            : HTMLInputElement.prototype;
      const setter = Object.getOwnPropertyDescriptor(proto, "value");
      if (setter && setter.set) setter.set.call(el, value);
      else el.value = value;
    }
    el.dispatchEvent(new Event("input", { bubbles: true }));
    el.dispatchEvent(new Event("change", { bubbles: true }));
    return "visible";
  };

  window.__shikisha_html = function () {
    return document.documentElement.outerHTML;
  };

  // Make the request from inside the page so we can read the status/body/
  // headers (the WebView doesn't expose raw HTTP directly, so we have the
  // page itself make the call and hand back the result). credentials:"include"
  // so logged-in cookies are used. Failures are returned as a value, not thrown
  window.__shikisha_fetch = async function (url, opts) {
    const o = opts || {};
    try {
      const r = await fetch(url, {
        method: o.method || "GET",
        headers: o.headers || undefined,
        body: o.body,
        credentials: "include",
        redirect: "follow",
      });
      let body = "";
      try { body = await r.text(); } catch (e) {}
      const MAX = 200000;
      let truncated = false;
      if (body.length > MAX) { body = body.slice(0, MAX); truncated = true; }
      const headers = {};
      r.headers.forEach(function (v, k) { headers[k] = v; });
      return { ok: r.ok, status: r.status, url: r.url, redirected: r.redirected,
               truncated: truncated, headers: headers, body: body };
    } catch (e) {
      return { ok: false, status: 0, error: String(e && e.message || e) };
    }
  };

  // ---- The Lua recorder ----------------------------------------------------
  // Turns what a human does on this page into calls of the very primitives the
  // automation uses (browser_fill / browser_click / browser_press). Semantic
  // events only: the committed value (change / Enter), and clicks on things
  // that aren't text fields. Only trusted input is recorded — the automation's
  // own synthetic events (isTrusted:false) are ignored, so a running script
  // never records itself, while relayed phone input (real CDP input) does.
  // Whether recording is on is remembered by the Rust side and re-issued on
  // every new document, exactly like the ask bar above.
  let recOn = false;
  window.__shikisha_rec = function (on) { recOn = !!on; };

  // Selector generation: readable first, unique always, durable when the site
  // allows it. A machine-generated id (Google's #ti6dpd, React's :r1:) changes
  // on every load, so anchoring to it records a selector that is dead by
  // tomorrow — such ids are refused and the stable attributes get their turn.
  const recEsc = (s) => (window.CSS && CSS.escape) ? CSS.escape(s) : s;
  const recUniq = (s) => { try { return document.querySelectorAll(s).length === 1; } catch (e) { return false; } };
  function recGenId(id) {
    if (id.indexOf(":") >= 0) return true;                    // React useId and kin
    if (/^(ember|yui_|ext-)/.test(id)) return true;           // framework counters
    if (/^[0-9a-f-]{8,}$/i.test(id) && /\d/.test(id)) return true;  // hex / uuid
    if (/^[A-Za-z0-9]{4,12}$/.test(id) && !/[_-]/.test(id)) {
      if (/\d/.test(id)) return true;                         // letter-digit mash
      const upper = (id.match(/[A-Z]/g) || []).length;
      const lower = (id.match(/[a-z]/g) || []).length;
      if (upper >= 2 && lower >= 2) return true;              // case-mash (APjFqb)
    }
    return false;
  }
  // A durable address: a human-made unique id, else a unique stable attribute.
  function recSelStable(el) {
    if (el.id && !recGenId(el.id)) { const s = "#" + recEsc(el.id); if (recUniq(s)) return s; }
    const tag = el.tagName.toLowerCase();
    for (const a of ["name", "aria-label", "placeholder", "data-testid"]) {
      const v = el.getAttribute(a);
      if (v) { const s = tag + "[" + a + "=" + JSON.stringify(v) + "]"; if (recUniq(s)) return s; }
    }
    return null;
  }
  // Last resort: a structural nth-of-type path, extended upward until unique.
  // Position-based, so it survives reloads but not layout changes.
  function recSelPath(el) {
    let s = "", cur = el;
    while (cur && cur.nodeType === 1 && cur.tagName !== "HTML") {
      const par = cur.parentElement;
      let seg;
      if (cur.id && !recGenId(cur.id)) seg = "#" + recEsc(cur.id);
      else {
        seg = cur.tagName.toLowerCase();
        if (par) {
          const same = Array.prototype.filter.call(par.children, (c) => c.tagName === cur.tagName);
          if (same.length > 1) seg += ":nth-of-type(" + (same.indexOf(cur) + 1) + ")";
        }
      }
      s = seg + (s ? " > " + s : "");
      if (recUniq(s)) return s;
      cur = par;
    }
    return s || el.tagName.toLowerCase();
  }
  function recSel(el) { return recSelStable(el) || recSelPath(el); }
  // The visible text, flattened to one line (an anchor and a human hint).
  function recText(el) {
    return (el.innerText || el.textContent || "").replace(/\s+/g, " ").trim();
  }
  // For clicks on things WITH a face (links, buttons): address them by their
  // visible text via XPath — the one anchor that survives both random ids and
  // layout reshuffles. Only when that text matches exactly one element.
  function recXpathByText(el) {
    const tag = el.tagName.toLowerCase();
    if (!/^(a|button|summary)$/.test(tag) && el.getAttribute("role") !== "button") return null;
    const t = recText(el);
    if (!t || t.length > 60 || t.indexOf('"') >= 0) return null;
    const xp = "//" + tag + "[normalize-space(.)=\"" + t + "\"]";
    try {
      const n = document.evaluate("count(" + xp + ")", document, null, 1, null).numberValue;
      return n === 1 ? xp : null;
    } catch (e) { return null; }
  }

  // Text-like editables commit on change/Enter; everything else commits on
  // click. A click that merely focuses a field isn't an action, so it's skipped.
  function recEditable(el) {
    if (!el || el.nodeType !== 1) return false;
    if (el.isContentEditable || el.tagName === "TEXTAREA") return true;
    return el.tagName === "INPUT" &&
      !/^(button|submit|reset|checkbox|radio|file|image|range|color)$/.test(el.type);
  }
  // Enter reports the fill itself (program order: value, then the key), so the
  // change event that follows the same commit must not report it again.
  let recLast = "";
  function recFill(el) {
    const sel = recSel(el);
    // Never the password itself — report a fill-from-secrets step instead
    if (el.tagName === "INPUT" && el.type === "password") {
      send({ kind: "recorded", act: "secret", sel: sel, value: "" });
      return;
    }
    const v = el.isContentEditable ? el.textContent : el.value;
    if (sel + "\n" + v === recLast) return;
    recLast = sel + "\n" + v;
    send({ kind: "recorded", act: "fill", sel: sel, value: v });
  }
  document.addEventListener("click", function (e) {
    if (!recOn || !e.isTrusted) return;
    let el = e.target;
    if (el && el.closest) el = el.closest("a,button,[role=button],input,select,summary,label") || el;
    if (!el || el.nodeType !== 1) return;
    if (recEditable(el) || el.tagName === "SELECT") return;
    // Durable CSS first; a text-anchored XPath beats a positional path; the
    // path travels with a human hint (the text) so a broken line can be
    // repaired by a person or an AI without re-recording.
    const stable = recSelStable(el);
    if (stable) {
      send({ kind: "recorded", act: "click", sel: stable, hint: recText(el).slice(0, 40) });
      return;
    }
    const byText = recXpathByText(el);
    if (byText) {
      send({ kind: "recorded", act: "click", sel: byText, xpath: true });
      return;
    }
    send({ kind: "recorded", act: "click", sel: recSelPath(el), hint: recText(el).slice(0, 40) });
  }, true);
  document.addEventListener("change", function (e) {
    if (!recOn || !e.isTrusted) return;
    const el = e.target;
    if (el.tagName === "SELECT") {
      send({ kind: "recorded", act: "fill", sel: recSel(el), value: el.value });
    } else if (recEditable(el)) {
      recFill(el);
    }
  }, true);
  document.addEventListener("keydown", function (e) {
    if (!recOn || !e.isTrusted || e.isComposing || e.key !== "Enter" || e.shiftKey) return;
    if (recEditable(e.target)) {
      recFill(e.target);
      send({ kind: "recorded", act: "press", sel: "", value: "enter" });
    }
  }, true);

  window.__shikisha = true;

  // "Loading finished" waits for `load`. At DOMContentLoaded, images and
  // CSS haven't arrived yet, and content JS builds afterward isn't in place.
  //
  // But ad-laden pages wait on external tracking tags, so `load` can lag
  // several seconds, or never fire. If we can't wait that long, announce
  // at the DOM-only point instead and record which case it was in
  // `complete`. Better to be honest than to guess and be wrong
  let told = false;
  const announce = complete => {
    if (told) return;
    told = true;
    send({ kind: "loading", busy: false });   // Loading finished = clear the "busy" indicator
    send({ kind: "ready", url: location.href, complete: !!complete });
  };
  const SETTLE_MS = 8000;
  if (document.readyState === "complete") {
    announce(true);
  } else {
    addEventListener("load", () => announce(true), { once: true });
    const armFallback = () => setTimeout(() => announce(false), SETTLE_MS);
    if (document.readyState === "loading") {
      document.addEventListener("DOMContentLoaded", armFallback, { once: true });
    } else {
      armFallback();
    }
  }
})();
"##;

/// An instruction from the conductor to the browser
#[derive(Debug, Clone)]
pub enum Cmd {
    /// Another address the app's own pages come from. The settings and the
    /// result view are served by a second local server whose port is only
    /// known once it starts, so it is told here rather than at the window's birth
    Trust { origin: String },
    /// Evaluate JS and return the result (matched up by `id`).
    /// `to` is the destination page name. `None` means the main view
    Eval {
        id: u64,
        to: Option<String>,
        js: String,
    },
    /// Call one CDP method and send its result back (matched up by `id`).
    /// The JS path (`Eval`) can't see the DevTools protocol, and CDP is
    /// where the accessibility tree, layout snapshot, and genuine input live
    Cdp {
        id: u64,
        to: Option<String>,
        method: String,
        params: String,
    },
    /// Place a named page inside the same window
    AddChild {
        name: String,
        url: String,
        rect: (i32, i32, i32, i32),
        /// This page's data storage (profile / private)
        profile: BrowserProfile,
    },
    /// Set the placed page's position and size. Width or height of 0 hides it
    ChildBounds {
        name: String,
        rect: (i32, i32, i32, i32),
    },
    /// Remove a placed page
    RemoveChild { name: String },
    /// Take in the windows pages asked for, and let go of the ones that asked
    /// to close. Sent by the handlers that answer those requests: they run on
    /// the message loop and cannot reach into the loop's own bookkeeping
    Adopt,
    /// Keep a hidden page's compositor running (`on=true`) for the duration
    /// of genuine-input operations, and release it again (`on=false`).
    ///
    /// A page that isn't on screen (bounds 0×0) stops compositing, and mouse
    /// input is the one kind that needs the compositor — its ack never comes.
    /// A tiny screencast forces frame production (the same mechanism the
    /// phone relay rides), so input lands and acks deterministically; the
    /// synchronization is the CDP completion itself, never a timer
    Wake { to: Option<String>, on: bool },
    /// Move keyboard focus to this page. `None` for `to` means the main view.
    ///
    /// Focus inside the page (activeElement) and the focus the OS sees are
    /// separate things. Showing/hiding a stacked page can leave the OS-level
    /// focus stranded elsewhere, which shows up as keystrokes arriving fine
    /// while the Japanese IME candidate window pops up in the wrong corner
    Focus { to: Option<String> },
    /// Move a placed page (the human pressed the bar above it)
    Move { to: Option<String>, go: Go },
    /// Ask where we currently are and whether we can go back/forward.
    /// The answer comes back as `Ev::Where`
    Where { to: Option<String> },
    /// Start/stop screencasting (VNC-equivalent).
    /// Once started, `Ev::Frame` arrives on every change. `to` is the target
    /// page (`None` is the main view)
    Screencast {
        to: Option<String>,
        on: bool,
    },
    /// Inject real input into the screencast target (via CDP — treated as
    /// genuine input, not synthetic). Both a human's finger trace and a
    /// CAPTCHA swipe are replayed exactly as the points arrive
    Inject {
        to: Option<String>,
        input: Input,
    },
    /// Arm basic auth. From then on, this page's 401 challenges get
    /// credentials returned via CDP (Fetch.authRequired -> continueWithAuth).
    /// user/pass are already resolved from secrets and are never handed to AI/Lua
    BasicAuth {
        to: Option<String>,
        user: String,
        pass: String,
    },
    /// Put the window away. The program, its tabs and the phone's connection
    /// go on; only the picture is gone, and the icon in the notification area
    /// is how it comes back
    Hide,
    /// Bring the window back in front of the person, from put away or minimised
    Show,
    /// A notice from the notification-area icon (a banner Windows draws)
    TrayNotice { title: String, text: String },
    /// Close the window (when the conductor is gone)
    Close,
}



/// any web page went into the terminal as if the person had typed it.
///
/// Not on the list: the press of the bar that asks the person something
/// (`Button`). A page cannot be believed about that -- the whole point of the
/// bar is that a person, not the page, said "done" -- which is why the bar is
/// drawn by the board, outside the page, and only the board reports the press
///
/// Written from the side that enumerates what gets through, like
/// `remote::allowed_from_afar` is for the phone. Add to it only after writing
/// down why a stranger's page needs it
/// Whether a page placed in the window — somebody else's page — may say this.
///
/// A placed page runs whatever script its site serves, and that script can
/// call `window.ipc.postMessage` exactly as ours do. So a page is let to
/// *report* — a step it recorded, that it is loading or has loaded, that it
/// took the focus or its pen was pressed, the answer to a question we put to
/// it — and never to *ask*: nothing here types into a tab, runs Lua, touches
/// git, or opens the settings. Before this list existed, `{kind:"say"}` from
pub fn allowed_from_page(ev: &Ev) -> bool {
    matches!(
        ev,
        Ev::Ready { .. }
            | Ev::Loading { .. }
            | Ev::Touched { .. }
            | Ev::Compose { .. }
            | Ev::Recorded { .. }
            | Ev::Result { .. }
    )
}

/// The part of an address that says which site a page belongs to: scheme,
/// host and port (the scheme's usual port when none is written). `None` for
/// anything that is not an address with a host
fn origin_of(addr: &str) -> Option<(String, String, u16)> {
    let uri: wry::http::Uri = addr.trim().parse().ok()?;
    let scheme = uri.scheme_str()?.to_ascii_lowercase();
    let host = uri.host()?.to_ascii_lowercase();
    let port = uri.port_u16().or(match scheme.as_str() {
        "http" => Some(80),
        "https" => Some(443),
        _ => None,
    })?;
    Some((scheme, host, port))
}

/// Whether two addresses belong to the same site. Judged by parsing both, not
/// by the front of the string: `http://127.0.0.1:8787.evil.example/` starts
/// with our address and is not ours
pub fn same_origin(a: &str, b: &str) -> bool {
    match (origin_of(a), origin_of(b)) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

/// Whether `at` is one of the app's own addresses (the board's, the settings
/// server's). Two servers, because the settings and the result view are
/// served by one that starts on its own port when first needed
fn from_ours(own: &[String], at: &str) -> bool {
    own.iter().any(|o| same_origin(at, o))
}


/// A result is believed only from the page it was asked of. Without this, any
/// placed page could answer for another — a stranger's tab filling in what
/// `browser_text` read from the tab beside it — by posting `{kind:"result"}`
/// with a guessed id (ids count up from one). Bounded, because a page that
/// navigates away mid-question never answers, and its entry would otherwise
/// stay for the life of the window
#[derive(Default)]
pub struct Asked {
    of: std::collections::HashMap<u64, Option<String>>,
    order: std::collections::VecDeque<u64>,
}

impl Asked {
    /// The most questions kept waiting for an answer at once
    const KEPT: usize = 4096;

    /// Remember that question `id` went to `to` (`None` is the board itself)
    pub fn ask(&mut self, id: u64, to: Option<String>) {
        if self.of.insert(id, to).is_none() {
            self.order.push_back(id);
        }
        while self.order.len() > Self::KEPT {
            if let Some(old) = self.order.pop_front() {
                self.of.remove(&old);
            }
        }
    }

    /// Whether `by` is who question `id` was put to. True once: the question
    /// is closed by its answer, so a second answer — from anyone — is refused
    pub fn answered(&mut self, id: u64, by: Option<&str>) -> bool {
        match self.of.get(&id) {
            Some(to) if to.as_deref() == by => {
                self.of.remove(&id);
                self.order.retain(|x| *x != id);
                true
            }
            _ => false,
        }
    }
}

/// Read what a page in the window said, and keep only what it may say.
///
/// `who` is the page's name (`None` for the board itself). `ours` is whether the
/// page is one of the app's own — judged by the address it spoke from, so the
/// settings page keeps its full voice while the same pane, navigated to another
/// site, loses it. A stranger is held to `allowed_from_page`; everyone's
/// answers are checked against `asked`; and the report's sender is stamped
/// here, never taken from the message, so no page can speak as another
pub fn heard(
    body: &str,
    who: Option<&str>,
    ours: bool,
    asked: &mut Asked,
) -> Option<Ev> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let ev = parse_intent(&v)?;
    if !ours && !allowed_from_page(&ev) {
        return None;
    }
    let from = who.map(str::to_string);
    Some(match ev {
        Ev::Result { id, ok, value } => {
            if !asked.answered(id, who) {
                return None;
            }
            Ev::Result { id, ok, value }
        }
        // Reports that say who sent them. A placed page's carry its name,
        // whatever the message claimed; the board's own carry none -- except
        // the bar's press, which names the page the bar stands under, and
        // only the board is heard on that (see `allowed_from_page`)
        Ev::Button { from: named } if who.is_none() => Ev::Button { from: named },
        Ev::Button { .. } => Ev::Button { from },
        Ev::Touched { .. } => Ev::Touched { from },
        Ev::Compose { .. } => Ev::Compose { from },
        Ev::Recorded { act, sel, value, xpath, hint, .. } => {
            Ev::Recorded { from, act, sel, value, xpath, hint }
        }
        Ev::Ready { url, complete, .. } => Ev::Ready { from, url, complete },
        Ev::Loading { busy, .. } => Ev::Loading { from, busy },
        other => other,
    })
}

/// A line in the log for a message the window refused, from whom and why.
/// Said a handful of times per page and then no more: a page that keeps
/// trying must not be able to fill the log
fn note_refused(said: &std::cell::Cell<u8>, who: Option<&str>, at: &str, body: &str) {
    if said.get() >= 5 {
        return;
    }
    said.set(said.get() + 1);
    let kind = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("kind").and_then(|k| k.as_str()).map(str::to_string))
        .unwrap_or_else(|| "?".into());
    // The site, not the whole address: a page's query string is its business
    let site = origin_of(at)
        .map(|(s, h, p)| format!("{s}://{h}:{p}"))
        .unwrap_or_else(|| "?".into());
    shikisha_core::append_hook_log(&format!(
        "[browser] refused '{kind}' from page '{}' at {site} (a placed page may report, not ask)",
        who.unwrap_or("(board)")
    ));
}



/// A handle to one running browser
pub struct Browser {
    proxy: tao::event_loop::EventLoopProxy<Cmd>,
    events: Receiver<Ev>,
    next_id: AtomicU64,
    /// The window is put away and the board's page dropped with it (see `hide`)
    away: std::sync::atomic::AtomicBool,
    /// Pages whose Lua recorder is armed (📼). The same navigation problem as
    /// the bar: the JS world (and its recOn flag) dies on every navigation, so
    /// membership here is what's true, re-issued per new document.
    pending_rec: std::sync::Mutex<std::collections::HashSet<Option<String>>>,
    /// A different signal that arrived while we were waiting on something.
    ///
    /// Skipping and discarding it means anything sent before the wait
    /// began vanishes forever. That's exactly how the window's column
    /// count once never arrived
    spare: std::sync::Mutex<Vec<Ev>>,
    /// The latest digest per page: position N-1 holds the backendNodeId
    /// behind `{ref=N}`. Cleared when that page navigates (backend ids die
    /// with the document, and a stale ref must say so, not click thin air)
    digests: std::sync::Mutex<std::collections::HashMap<Option<String>, Vec<i64>>>,
}

/// Is this a URL we're allowed to open? Only http/https pass.
///
/// When wry receives IPC from a page, it builds that page's URL as an
/// `http::Uri` and `unwrap`s it (webview2/mod.rs). Both `file:///` and
/// `data:` fail to parse there and **take down the whole process**
/// (confirmed by testing). Since the initialization script we inject
/// always sends IPC, opening one of these guarantees a crash. So we
/// stop it at the door.
///
/// To show a local file, serve it over this app's own local HTTP server
/// instead — it achieves the same thing
pub fn is_openable(url: &str) -> bool {
    let u = url.trim();
    let scheme_ok = u.starts_with("https://") || u.starts_with("http://");
    let has_host = u.split("//").nth(1).is_some_and(|rest| {
        let host = rest.split(['/', '?', '#']).next().unwrap_or("");
        !host.is_empty()
    });
    scheme_ok && has_host && !u.contains(['\n', '\r', ' '])
}



/// Where browser data lives — every last byte of it, under the one folder the
/// config names (`browser_data`). WebView2's store is heavy SQLite and cache, so
/// the default keeps it out of a Drive-synced folder.
///
/// One root matters more than it looks: each page is handed its own folder under
/// here, and that folder is the ONLY thing that separates one page's cookies from
/// another's. Point two pages at the same folder and they are the same visitor.
fn profiles_root() -> std::path::PathBuf {
    shikisha_core::config::browser_data_dir()
}

/// The window's own shell page (tab bar, board, terminal). It is our HTML, not
/// the web, so it shares nothing with the pages placed inside it — and it still
/// needs a folder of its own, or WebView2 drops one beside the exe
pub fn shell_data_dir() -> std::path::PathBuf {
    let dir = profiles_root().join("shell");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Turn a profile name into a safe folder name (strips path separators and `..`). Empty becomes "default"
fn sanitize_profile(name: &str) -> String {
    let s: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        .collect();
    let s = s.trim_matches('.').to_string();
    if s.is_empty() { "default".into() } else { s }
}

/// A running counter for private temp folder names (avoids collisions even within the same millisecond)
static PRIVATE_SEQ: AtomicU64 = AtomicU64::new(0);

/// Return the data folder for a profile spec (creating it too).
/// For private mode, a unique temp folder (a different one on every call)
fn profile_dir(p: &BrowserProfile) -> std::path::PathBuf {
    let dir = if p.private {
        let ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let n = PRIVATE_SEQ.fetch_add(1, Ordering::Relaxed);
        profiles_root().join("_private").join(format!("{ms:013}-{n:04}"))
    } else {
        profiles_root().join("profiles").join(sanitize_profile(&p.name))
    };
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Delete a closed private page's folder as soon as WebView2 lets go of it.
///
/// "Vanishes on close" is the whole promise of private mode, and the folder now
/// holds real cookies — so it can't be left lying about until the next launch.
/// WebView2 keeps its files locked for a moment after the view is gone, and the
/// window's own thread must not sit and wait (it pumps every message the window
/// gets), so the waiting happens off to the side. Startup's sweep is still the
/// backstop for anything this misses — a kill, a crash, a stubborn lock.
fn erase_when_released(dir: std::path::PathBuf) {
    std::thread::spawn(move || {
        for _ in 0..20 {
            if std::fs::remove_dir_all(&dir).is_ok() || !dir.exists() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
    });
}

/// At startup, sweep away any private areas left behind by a previous
/// abnormal exit. Private mode is supposed to "vanish on close", so
/// anything still there is garbage
pub fn sweep_private() {
    let _ = std::fs::remove_dir_all(profiles_root().join("_private"));
}

#[cfg(test)]
mod profile_tests {
    use super::*;

    /// Two pages are the same visitor exactly when they are handed the same
    /// folder. Nothing else separates them, so this is worth pinning down.
    #[test]
    fn every_profile_gets_its_own_folder() {
        let a = profile_dir(&BrowserProfile::new("work", false));
        let b = profile_dir(&BrowserProfile::new("home", false));
        let same = profile_dir(&BrowserProfile::new("work", false));
        assert_ne!(a, b, "別プロファイルが同じ入れ物を使っている");
        assert_eq!(a, same, "同じ名前なら同じ入れ物 (ログインが残る)");
        // Private is a fresh area EVERY time, which is what makes reopening one a
        // reset rather than a reload — the site meets someone it has never seen
        let p1 = profile_dir(&BrowserProfile::new("", true));
        let p2 = profile_dir(&BrowserProfile::new("", true));
        assert_ne!(p1, p2, "プライベートが同じ入れ物を使い回している");
        assert!(p1.starts_with(profiles_root().join("_private")), "掃除の対象から外れている: {p1:?}");
        // The shell is not one of the profiles, and never collides with a named one
        assert_ne!(shell_data_dir(), a);
        assert_ne!(shell_data_dir(), profile_dir(&BrowserProfile::shared_default()));
    }
}

/// The WebView2 runtime on this machine, if it has one.
///
/// Every window this program opens is a WebView2, so a machine without the
/// runtime cannot show anything at all -- including the message saying why.
/// That is why this is asked before the first window rather than discovered
/// through its failure: by then there is nowhere left to say it.
///
/// Windows 11 carries the runtime in the box. Windows 10 does not, and neither
/// does an image somebody has stripped, which is where this was found.
pub fn runtime_version() -> Option<String> {
    use webview2_com::Microsoft::Web::WebView2::Win32::GetAvailableCoreWebView2BrowserVersionString;
    use windows::core::{PCWSTR, PWSTR};

    let mut raw = PWSTR::null();
    unsafe { GetAvailableCoreWebView2BrowserVersionString(PCWSTR::null(), &mut raw) }.ok()?;
    let v = webview2_com::take_pwstr(raw);
    (!v.is_empty()).then_some(v)
}

impl Browser {
    /// Open the window and get it ready to accept instructions
    pub fn spawn(url: &str, title: &str) -> Result<Self> {
        if !is_openable(url) {
            return Err(anyhow!(shikisha_core::i18n::tp("err.browser.bad_url", &[("url", url)])));
        }
        Self::start(url, title)
    }

    fn start(url: &str, title: &str) -> Result<Self> {
        let (proxy_tx, proxy_rx) = channel();
        let (ev_tx, ev_rx) = channel();
        let url = url.to_string();
        let title = title.to_string();

        std::thread::Builder::new()
            .name("shikisha-browser".into())
            .spawn(move || {
                if let Err(e) = run_window(&url, &title, proxy_tx, ev_tx.clone()) {
                    shikisha_core::append_hook_log(&shikisha_core::i18n::tp(
                        "err.browser.log_open_failed",
                        &[("e", &format!("{e}"))],
                    ));
                    let _ = ev_tx.send(Ev::Closed);
                }
            })?;

        // Wait until the window exists (if it can't be created, the proxy never arrives)
        let proxy = proxy_rx
            .recv_timeout(std::time::Duration::from_secs(20))
            .map_err(|_| anyhow!(shikisha_core::i18n::t("err.browser.startup_timeout")))?;

        let me = Self {
            proxy,
            events: ev_rx,
            next_id: AtomicU64::new(1),
            away: std::sync::atomic::AtomicBool::new(false),
            pending_rec: std::sync::Mutex::new(std::collections::HashSet::new()),
            spare: std::sync::Mutex::new(Vec::new()),
            digests: std::sync::Mutex::new(std::collections::HashMap::new()),
        };
        // Don't return until the document is ready. Returning as soon as
        // the window exists would leave the caller touching an empty
        // document, unable to tell "the selector is wrong" from "it just
        // hasn't arrived yet"
        me.wait_ready(std::time::Duration::from_secs(30))?;
        Ok(me)
    }

    /// Wait until the next document is ready and return its URL.
    /// Fires once per navigation, so this is also used after `open`
    pub fn wait_ready(&self, timeout: std::time::Duration) -> Result<String> {
        let until = std::time::Instant::now() + timeout;
        loop {
            let left = until
                .checked_duration_since(std::time::Instant::now())
                .ok_or_else(|| anyhow!(shikisha_core::i18n::t("err.browser.page_not_ready")))?;
            match self.events.recv_timeout(left) {
                Ok(Ev::Ready { from, url, .. }) => {
                    self.reask(from.as_deref());
                    return Ok(url);
                }
                Ok(Ev::Closed) => return Err(anyhow!(shikisha_core::i18n::t("err.browser.closed"))),
                Ok(other) => {
                    self.spare.lock().unwrap().push(other);
                    continue;
                }
                Err(_) => return Err(anyhow!(shikisha_core::i18n::t("err.browser.page_not_ready"))),
            }
        }
    }

    fn send(&self, cmd: Cmd) -> Result<()> {
        self.proxy
            .send_event(cmd)
            .map_err(|_| anyhow!(shikisha_core::i18n::t("err.browser.not_connected")))
    }

    /// Put the window away (see `Cmd::Hide`). From here until `show`, JS for
    /// the main view is refused at this end: the page is gone, and sending
    /// every screen change to it would be a message per keystroke into nothing
    pub fn hide(&self) -> Result<()> {
        self.away.store(true, Ordering::Relaxed);
        self.send(Cmd::Hide)
    }

    /// Bring the window back in front of the person
    pub fn show(&self) -> Result<()> {
        self.away.store(false, Ordering::Relaxed);
        self.send(Cmd::Show)
    }

    /// Say that pages from this address are the app's own and are heard in
    /// full (see `heard`). The board's own address is trusted from the start;
    /// the settings server's is told here once it has started
    pub fn trust(&self, url: &str) -> Result<()> {
        self.send(Cmd::Trust { origin: url.to_string() })
    }

    /// A banner from the notification-area icon
    pub fn tray_notice(&self, title: &str, text: &str) -> Result<()> {
        self.send(Cmd::TrayNotice { title: title.to_string(), text: text.to_string() })
    }

    /// Evaluate JS. The result arrives later as `Ev::Result`
    pub fn eval(&self, js: &str) -> Result<u64> {
        self.eval_in(None, js)
    }

    /// Evaluate JS against a target. `None` is the main view
    pub fn eval_in(&self, to: Option<&str>, js: &str) -> Result<u64> {
        if to.is_none() && self.away.load(Ordering::Relaxed) {
            return Err(anyhow!(shikisha_core::i18n::t("err.browser.page_not_placed")));
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.send(Cmd::Eval {
            id,
            to: to.map(str::to_string),
            js: js.to_string(),
        })?;
        Ok(id)
    }

    /// Navigate a placed page
    pub fn go(&self, to: Option<&str>, go: Go) -> Result<()> {
        self.send(Cmd::Move {
            to: to.map(str::to_string),
            go,
        })
    }

    /// Start/stop screencasting (VNC-equivalent). Once started, `Ev::Frame` arrives
    pub fn screencast(&self, to: Option<&str>, on: bool) -> Result<()> {
        self.send(Cmd::Screencast {
            to: to.map(str::to_string),
            on,
        })
    }

    /// Inject input into the screencast view (finger traces, swipes, text)
    pub fn inject(&self, to: Option<&str>, input: Input) -> Result<()> {
        self.send(Cmd::Inject {
            to: to.map(str::to_string),
            input,
        })
    }

    /// Arm basic auth. From then on, returns credentials for this page's
    /// 401s. user/pass are already resolved from secrets (only the caller touches them)
    pub fn basic_auth(&self, to: Option<&str>, user: &str, pass: &str) -> Result<()> {
        self.send(Cmd::BasicAuth {
            to: to.map(str::to_string),
            user: user.to_string(),
            pass: pass.to_string(),
        })
    }

    /// Move keyboard focus (`None` = main view)
    pub fn focus(&self, to: Option<&str>) -> Result<()> {
        self.send(Cmd::Focus {
            to: to.map(str::to_string),
        })
    }

    /// Ask where we currently are (the answer arrives as a report)
    pub fn ask_where(&self, to: Option<&str>) -> Result<()> {
        self.send(Cmd::Where {
            to: to.map(str::to_string),
        })
    }

    /// Place a page inside the same window.
    ///
    /// Using a separate window would make ownership, position tracking,
    /// and even exposure during Windows Terminal tab switching all our
    /// own problem to manage. Placing it in the same window sidesteps all of it
    pub fn open_child(
        &self,
        name: &str,
        url: &str,
        rect: (i32, i32, i32, i32),
        profile: BrowserProfile,
    ) -> Result<()> {
        if !is_openable(url) {
            return Err(anyhow!(shikisha_core::i18n::tp("err.browser.bad_url", &[("url", url)])));
        }
        self.send(Cmd::AddChild {
            name: name.to_string(),
            url: url.to_string(),
            rect,
            profile,
        })
    }

    /// The placed page's position and size. Setting width or height to 0 hides it
    pub fn child_bounds(&self, name: &str, rect: (i32, i32, i32, i32)) -> Result<()> {
        self.send(Cmd::ChildBounds {
            name: name.to_string(),
            rect,
        })
    }

    pub fn close_child(&self, name: &str) -> Result<()> {
        self.send(Cmd::RemoveChild {
            name: name.to_string(),
        })
    }

    /// Call JS once and wait for the result
    fn call(
        &self,
        to: Option<&str>,
        func: &str,
        args: &[serde_json::Value],
        timeout_ms: u64,
    ) -> Result<String> {
        let id = self.eval_in(to, &call_js(func, args))?;
        self.wait_result(id, std::time::Duration::from_millis(timeout_ms))
    }

    /// Where that element currently is
    pub fn find(&self, to: Option<&str>, sel: &Sel, timeout_ms: u64) -> Result<Found> {
        if let Sel::Ref(r) = sel {
            return self.find_ref(to, *r, timeout_ms);
        }
        Ok(Found::parse(&self.call(
            to,
            "__shikisha_state",
            &[sel.json()],
            timeout_ms,
        )?))
    }

    /// Read text (an input field's contents, or the displayed string otherwise)
    /// The address this page is at now.
    ///
    /// Read from the page itself rather than remembered from when it was
    /// opened, because a page navigates -- a sign-in that hands off to another
    /// site, a link, a redirect -- and the address that matters when a
    /// password is about to be typed is the one on screen at that moment
    pub fn href(&self, to: Option<&str>, timeout_ms: u64) -> Result<String> {
        let v = self.call(to, "__shikisha_href", &[], timeout_ms)?;
        Ok(serde_json::from_str::<String>(&v).unwrap_or_default())
    }

    pub fn text(&self, to: Option<&str>, sel: &Sel, timeout_ms: u64) -> Result<Option<String>> {
        if let Sel::Ref(r) = sel {
            return self.text_ref(to, *r, timeout_ms);
        }
        let v = self.call(to, "__shikisha_text", &[sel.json()], timeout_ms)?;
        Ok(serde_json::from_str::<Option<String>>(&v).unwrap_or(None))
    }

    /// Run one auto-waiting in-page action (`__shikisha_click` / `_fill`) in
    /// slices until `timeout_ms` is spent.
    ///
    /// The in-page half of auto-wait (rAF polling) dies with its document,
    /// so a single long wait would keep polling a page that navigation
    /// already replaced. Short slices re-enter the *current* document each
    /// time. A slice that errors (context destroyed mid-navigation) or
    /// times out is retried while time remains
    fn act_with_wait(
        &self,
        to: Option<&str>,
        func: &str,
        mut args: Vec<serde_json::Value>,
        timeout_ms: u64,
    ) -> Result<Found> {
        const SLICE_MS: u64 = 1_200;
        // The JS answers a bit before the slice so the result beats the wait
        const CUSHION_MS: u64 = 300;
        let until = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
        let mut last_err: Option<anyhow::Error> = None;
        loop {
            let left = until
                .saturating_duration_since(std::time::Instant::now())
                .as_millis() as u64;
            if left < CUSHION_MS + 100 {
                return match last_err {
                    Some(e) => Err(e),
                    None => Ok(Found::NotFound),
                };
            }
            let slice = left.min(SLICE_MS);
            args.push(serde_json::json!(slice - CUSHION_MS));
            let res = self.call(to, func, &args, slice + CUSHION_MS);
            args.pop();
            match res {
                Ok(v) => match Found::parse(&v) {
                    Found::NotFound => {
                        last_err = None;
                        continue;
                    }
                    found => return Ok(found),
                },
                // Mid-navigation the evaluation context dies — that's the
                // moment auto-wait exists for, not a failure yet. Pace the
                // re-entry so a page stuck erroring doesn't get hammered
                Err(e) => {
                    last_err = Some(e);
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    continue;
                }
            }
        }
    }

    /// Click it. A `{ref=N}` clicks with a genuine (trusted) mouse event and
    /// reports what was clicked plus a durable anchor; selectors keep the
    /// synthetic in-page `el.click()` and report the state alone. Both paths
    /// auto-wait for the element to appear and settle (see `act_with_wait`)
    pub fn click(&self, to: Option<&str>, sel: &Sel, timeout_ms: u64) -> Result<OpReport> {
        if let Sel::Ref(r) = sel {
            return self.click_ref(to, *r, timeout_ms);
        }
        Ok(OpReport::bare(self.act_with_wait(
            to,
            "__shikisha_click",
            vec![sel.json()],
            timeout_ms,
        )?))
    }

    /// Put a value into an input field. A `{ref=N}` types genuine key events
    /// and reports which field was written; selectors keep the value-setter
    /// route. Both paths auto-wait (see `act_with_wait`)
    pub fn fill(
        &self,
        to: Option<&str>,
        sel: &Sel,
        value: &str,
        timeout_ms: u64,
    ) -> Result<OpReport> {
        if let Sel::Ref(r) = sel {
            return self.fill_ref(to, *r, value, timeout_ms);
        }
        Ok(OpReport::bare(self.act_with_wait(
            to,
            "__shikisha_fill",
            vec![sel.json(), serde_json::Value::String(value.to_string())],
            timeout_ms,
        )?))
    }

    /// The full parsed HTML
    pub fn html(&self, to: Option<&str>, timeout_ms: u64) -> Result<String> {
        let v = self.call(to, "__shikisha_html", &[], timeout_ms)?;
        Ok(serde_json::from_str::<String>(&v).unwrap_or(v))
    }

    /// Every cookie this page's profile holds, as the browser itself reports
    /// them.
    ///
    /// Read through the DevTools protocol rather than from a page script,
    /// because that is the only place the httpOnly cookies live -- and those
    /// are exactly the ones a login is made of. What comes back is the
    /// browser's own list, kept as-is so that loading it again asks for
    /// nothing to be reconstructed
    pub fn cookies_out(&self, to: Option<&str>, timeout_ms: u64) -> Result<serde_json::Value> {
        let v = self.cdp(to, "Network.getAllCookies", serde_json::json!({}), timeout_ms)?;
        Ok(v.get("cookies").cloned().unwrap_or(serde_json::Value::Array(vec![])))
    }

    /// A picture of the page as it looks right now, as PNG bytes.
    ///
    /// Taken by the browser itself through the devtools protocol, so it is what
    /// a person would see, not a re-render of the HTML. For a rally to keep a
    /// visual record of what it did, or for a person to glance at where an
    /// agent got to without switching to the tab
    pub fn snapshot(&self, to: Option<&str>, timeout_ms: u64) -> Result<Vec<u8>> {
        use base64::Engine as _;
        let v = self.cdp(
            to,
            "Page.captureScreenshot",
            serde_json::json!({ "format": "png", "captureBeyondViewport": false }),
            timeout_ms,
        )?;
        let data = v
            .get("data")
            .and_then(|d| d.as_str())
            .ok_or_else(|| anyhow!(shikisha_core::i18n::t("err.browser.no_snapshot")))?;
        base64::engine::general_purpose::STANDARD
            .decode(data.as_bytes())
            .map_err(|e| anyhow!(shikisha_core::i18n::tp("err.browser.no_snapshot_decode", &[("e", &e.to_string())])))
    }

    /// Put a set of cookies back into this page's profile.
    ///
    /// The same shape that came out. Set against the live profile, so a page
    /// that reloads afterwards is simply logged in -- there was never a moment
    /// where our code decided what "logged in" meant
    pub fn cookies_in(
        &self,
        to: Option<&str>,
        cookies: &serde_json::Value,
        timeout_ms: u64,
    ) -> Result<()> {
        self.cdp(
            to,
            "Network.setCookies",
            serde_json::json!({ "cookies": cookies }),
            timeout_ms,
        )?;
        Ok(())
    }

    /// This page's localStorage, as `[[key, value], ...]`.
    ///
    /// Read in the page's own world through the devtools protocol, so it is the
    /// origin's real storage -- where a modern web app often keeps the token
    /// that says you are signed in, the half a cookie does not hold. Empty when
    /// the page has none or is not one that has storage (a blank tab)
    pub fn storage_out(&self, to: Option<&str>, timeout_ms: u64) -> Result<serde_json::Value> {
        let v = self.cdp(
            to,
            "Runtime.evaluate",
            serde_json::json!({
                // Guarded: a page mid-navigation, or one that denies storage,
                // must answer with nothing rather than throw
                "expression": "(()=>{try{return JSON.stringify(Object.entries(localStorage))}catch(e){return \"[]\"}})()",
                "returnByValue": true,
            }),
            timeout_ms,
        )?;
        let raw = v.get("result").and_then(|r| r.get("value")).and_then(|s| s.as_str());
        Ok(raw
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or(serde_json::Value::Array(vec![])))
    }

    /// Put localStorage back into this page's origin.
    ///
    /// Set in the page's own world, the same way it was read. A reload
    /// afterwards is what makes the app notice it and consider itself signed in
    pub fn storage_in(
        &self,
        to: Option<&str>,
        items: &serde_json::Value,
        timeout_ms: u64,
    ) -> Result<()> {
        let payload = serde_json::to_string(items).unwrap_or_else(|_| "[]".into());
        let expr = format!(
            "(()=>{{try{{for(const [k,v] of {payload}){{localStorage.setItem(k,v)}}return true}}catch(e){{return false}}}})()"
        );
        self.cdp(
            to,
            "Runtime.evaluate",
            serde_json::json!({ "expression": expr, "returnByValue": true }),
            timeout_ms,
        )?;
        Ok(())
    }

    /// Make a request from inside the page. Returns a JSON string
    /// `{status,ok,url,headers,body,...}`.
    /// `opts` is `{method,headers,body}` (optional)
    pub fn fetch(
        &self,
        to: Option<&str>,
        url: &str,
        opts: &serde_json::Value,
        timeout_ms: u64,
    ) -> Result<String> {
        self.call(
            to,
            "__shikisha_fetch",
            &[serde_json::Value::String(url.to_string()), opts.clone()],
            timeout_ms,
        )
    }

    /// Drain the reports accumulated so far (doesn't block).
    /// If we moved to a new document, re-show the bar that should be showing
    pub fn drain(&self) -> Vec<Ev> {
        // Return anything that arrived while we were waiting first (preserves arrival order)
        let mut evs: Vec<Ev> = std::mem::take(&mut *self.spare.lock().unwrap());
        evs.extend(self.events.try_iter());
        for e in &evs {
            if let Ev::Ready { from, .. } = e {
                self.reask(from.as_deref());
            }
        }
        evs
    }

    /// Arm/disarm the Lua recorder (📼) on a page. Like the bar, "should it be
    /// recording" lives here and is re-issued on every new document.
    pub fn record(&self, to: Option<&str>, on: bool) -> Result<()> {
        let key = to.map(str::to_string);
        if on {
            self.pending_rec.lock().unwrap().insert(key);
        } else {
            self.pending_rec.lock().unwrap().remove(&key);
        }
        self.eval_in(to, &format!("window.__shikisha_rec && window.__shikisha_rec({on});"))
            .map(|_| ())
    }

    /// Silence the recorder everywhere (there's only ever one recorder — arming
    /// a page goes through this first, so two pages never record at once)
    pub fn record_all_off(&self) {
        let keys: Vec<Option<String>> =
            self.pending_rec.lock().unwrap().drain().collect();
        for k in keys {
            let _ = self.eval_in(
                k.as_deref(),
                "window.__shikisha_rec && window.__shikisha_rec(false);",
            );
        }
    }

    /// Re-dress a document that navigation just wiped: the recorder arming is
    /// Rust-remembered state, re-issued per new document. Only for the page
    /// that navigated. (The bar asking the person something is not in the
    /// page any more, so it needs no re-dressing: it stays up on the board
    /// until the script takes it down)
    fn reask(&self, to: Option<&str>) {
        let key = to.map(str::to_string);
        // The digest died with the document (backendNodeIds are per-document).
        // Dropping it here turns a later `{ref=N}` into a clear "take a new
        // digest" instead of a click on a node that no longer exists
        self.digests.lock().unwrap().remove(&key);
        if self.pending_rec.lock().unwrap().contains(&key) {
            let _ = self.eval_in(to, "window.__shikisha_rec && window.__shikisha_rec(true);");
        }
    }

    /// Wait until a password is entered.
    /// Any other signal that arrives while waiting is kept aside (discarding it loses it forever)
    pub fn wait_password(&self, timeout: std::time::Duration) -> Result<Option<String>> {
        let until = std::time::Instant::now() + timeout;
        loop {
            let left = until
                .checked_duration_since(std::time::Instant::now())
                .ok_or_else(|| anyhow!(shikisha_core::i18n::t("err.browser.no_input")))?;
            match self.events.recv_timeout(left) {
                Ok(Ev::Password { text }) => return Ok(text),
                Ok(Ev::Closed) => return Err(anyhow!(shikisha_core::i18n::t("err.browser.window_closed"))),
                Ok(other) => {
                    self.spare.lock().unwrap().push(other);
                    continue;
                }
                Err(_) => return Err(anyhow!(shikisha_core::i18n::t("err.browser.no_input"))),
            }
        }
    }

    /// Wait until a specific evaluation's result arrives
    pub fn wait_result(&self, id: u64, timeout: std::time::Duration) -> Result<String> {
        let (ok, value) = self.wait_ev(id, timeout)?;
        if ok {
            Ok(value)
        } else {
            Err(anyhow!(shikisha_core::i18n::tp(
                "err.browser.js_eval_failed",
                &[("value", &value)]
            )))
        }
    }

    /// The shared wait behind `Eval` and `Cdp`: the raw (ok, payload) pair for
    /// one id, so each caller can word its own failure
    fn wait_ev(&self, id: u64, timeout: std::time::Duration) -> Result<(bool, String)> {
        let until = std::time::Instant::now() + timeout;
        loop {
            let left = until
                .checked_duration_since(std::time::Instant::now())
                .ok_or_else(|| anyhow!(shikisha_core::i18n::t("err.browser.no_result")))?;
            match self.events.recv_timeout(left) {
                Ok(Ev::Result { id: got, ok, value }) if got == id => return Ok((ok, value)),
                Ok(Ev::Ready { from, .. }) => {
                    self.reask(from.as_deref());
                    continue;
                }
                Ok(other) => {
                    self.spare.lock().unwrap().push(other);
                    continue;
                }
                Err(_) => return Err(anyhow!(shikisha_core::i18n::t("err.browser.no_result"))),
            }
        }
    }

    // ── CDP-backed operations (digest and {ref=N}) ──────────────────────
    //
    // The JS world can only see what a page chooses to expose; the DevTools
    // protocol sees what the browser itself knows (accessibility tree, layout,
    // and genuine input injection). The digest and every ref operation live on
    // this side so that names come from the browser's accname computation and
    // clicks/keys are real input events, indistinguishable from a human's.

    /// Call one CDP method on a page and wait for its result (parsed JSON)
    fn cdp(
        &self,
        to: Option<&str>,
        method: &str,
        params: serde_json::Value,
        timeout_ms: u64,
    ) -> Result<serde_json::Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.send(Cmd::Cdp {
            id,
            to: to.map(str::to_string),
            method: method.to_string(),
            params: params.to_string(),
        })?;
        let (ok, value) = self.wait_ev(id, std::time::Duration::from_millis(timeout_ms))?;
        if !ok {
            return Err(anyhow!(shikisha_core::i18n::tp(
                "err.browser.cdp_failed",
                &[("method", method), ("e", &value)]
            )));
        }
        Ok(serde_json::from_str(&value).unwrap_or(serde_json::Value::Null))
    }

    /// Distill the page into its operable elements (see `shikisha_core::digest`), and
    /// remember the ref-number → backendNodeId mapping for `{ref=N}` calls
    pub fn digest(&self, to: Option<&str>, timeout_ms: u64) -> Result<String> {
        let metrics = self.cdp(to, "Page.getLayoutMetrics", serde_json::json!({}), timeout_ms)?;
        let snap = self.cdp(
            to,
            "DOMSnapshot.captureSnapshot",
            serde_json::json!({ "computedStyles": ["cursor"] }),
            timeout_ms,
        )?;
        // Roles and accessible names, as the browser itself computed them
        let ax = self.cdp(
            to,
            "Accessibility.getFullAXTree",
            serde_json::json!({}),
            timeout_ms,
        )?;
        let d = shikisha_core::digest::build(&ax, &snap, &metrics);
        self.digests
            .lock()
            .unwrap()
            .insert(to.map(str::to_string), d.refs);
        Ok(d.text)
    }

    /// Resolve `{ref=N}` against the latest digest of that page
    fn ref_backend(&self, to: Option<&str>, r: u32) -> Result<i64> {
        let map = self.digests.lock().unwrap();
        let refs = map
            .get(&to.map(str::to_string))
            .ok_or_else(|| anyhow!(shikisha_core::i18n::t("err.browser.ref_no_digest")))?;
        (r as usize)
            .checked_sub(1)
            .and_then(|i| refs.get(i))
            .copied()
            .ok_or_else(|| {
                anyhow!(shikisha_core::i18n::tp(
                    "err.browser.ref_unknown",
                    &[("ref", &r.to_string()), ("max", &refs.len().to_string())]
                ))
            })
    }

    /// Word a CDP failure on a ref as what it almost always is: the element
    /// (or the whole document) is gone since the digest was taken
    fn ref_stale(r: u32, e: anyhow::Error) -> anyhow::Error {
        anyhow!(shikisha_core::i18n::tp(
            "err.browser.ref_stale",
            &[("ref", &r.to_string()), ("e", &e.to_string())]
        ))
    }

    /// The center of the first non-degenerate content quad, in viewport CSS px
    fn quad_center(q: &serde_json::Value) -> Option<(f64, f64)> {
        for quad in q.get("quads")?.as_array()? {
            let p: Vec<f64> = quad
                .as_array()?
                .iter()
                .filter_map(serde_json::Value::as_f64)
                .collect();
            if p.len() == 8 {
                let x = (p[0] + p[2] + p[4] + p[6]) / 4.0;
                let y = (p[1] + p[3] + p[5] + p[7]) / 4.0;
                // A zero-area quad is a collapsed (invisible) box
                if (p[0] - p[2]).abs() + (p[1] - p[7]).abs() > 0.5 {
                    return Some((x, y));
                }
            }
        }
        None
    }

    /// Ask the window to keep this page's compositor running (or release it).
    /// Fire-and-forget: the command channel preserves order, and the input
    /// call that follows synchronizes on its own CDP completion
    fn wake(&self, to: Option<&str>, on: bool) {
        let _ = self.send(Cmd::Wake {
            to: to.map(str::to_string),
            on,
        });
    }

    /// Click a digest ref with genuine mouse events. Returns the state plus
    /// an echo — what was actually clicked (`link 「…」`) — so a wrong ref
    /// number is exposed by its own answer instead of failing silently.
    ///
    /// A hidden webview (bounds 0×0 — e.g. another tab is showing) stops
    /// compositing, and mouse events are the one input kind that needs the
    /// compositor — their ack never arrives. The wake (an off-client-area
    /// surface plus a tiny throwaway screencast) forces frames back on for
    /// the duration of the click, so genuine input lands whether or not the
    /// page is on screen. The synchronization is the CDP completion itself —
    /// no timers. Should input still not land (unknown edge), the element's
    /// own `click()` is the last-resort fallback rather than a dead move
    fn click_ref(&self, to: Option<&str>, r: u32, timeout_ms: u64) -> Result<OpReport> {
        self.wake(to, true);
        let out = self.click_ref_inner(to, r, timeout_ms);
        self.wake(to, false);
        out
    }

    /// A durable, digest-free address for the element behind `oid`, derived
    /// from the element itself at the moment it was touched. Priority: a
    /// human-made unique id, a unique text anchor, a unique stable attribute,
    /// then — when a candidate matches several elements (Google keeps two
    /// btnK buttons, result links repeat their href) — the same candidate
    /// pinned to this element's position, `(xpath)[k]`. Last resort is the
    /// 📼 recorder's structural nth-of-type path. Machine-minted ids are
    /// refused (recorder hygiene). None only when the element is beyond a
    /// selector's reach at all (shadow DOM) — the journal says so rather
    /// than record a lie
    fn element_anchor(
        &self,
        to: Option<&str>,
        oid: &str,
        timeout_ms: u64,
    ) -> Option<(String, String)> {
        const ANCHOR: &str = r##"function () {
            const uniqCss = (s) => { try { return document.querySelectorAll(s).length === 1; } catch (e) { return false; } };
            // -1 = unique and it's me; k>0 = me at position k of several; 0 = no use
            const place = (xp) => {
                try {
                    const r = document.evaluate(xp, document, null, 7, null);
                    if (r.snapshotLength === 1) return r.snapshotItem(0) === this ? -1 : 0;
                    for (let i = 0; i < r.snapshotLength; i++) {
                        if (r.snapshotItem(i) === this) return i + 1;
                    }
                } catch (e) {}
                return 0;
            };
            // XPath string literals can hold either quote kind, not both
            const xq = (s) => !s.includes('"') ? '"' + s + '"' : (!s.includes("'") ? "'" + s + "'" : null);
            const generated = (id) => {
                if (id.indexOf(":") >= 0) return true;
                if (/^(ember|yui_|ext-)/.test(id)) return true;
                if (/^[0-9a-f-]{8,}$/i.test(id) && /\d/.test(id)) return true;
                if (/^[A-Za-z0-9]{4,12}$/.test(id) && !/[_-]/.test(id)) {
                    if (/\d/.test(id)) return true;
                    const u = (id.match(/[A-Z]/g) || []).length;
                    const l = (id.match(/[a-z]/g) || []).length;
                    if (u >= 2 && l >= 2) return true;
                }
                return false;
            };
            const esc = (s) => (window.CSS && CSS.escape) ? CSS.escape(s) : s;
            const id = this.id || "";
            if (id && !generated(id) && uniqCss("#" + esc(id))) {
                return JSON.stringify({ kind: "css", v: "#" + id });
            }
            const tag = this.tagName.toLowerCase();
            const cands = [];
            const txt = (this.innerText || "").replace(/\s+/g, " ").trim();
            if (txt && txt.length <= 60) {
                const q = xq(txt);
                if (q) cands.push("//" + tag + "[normalize-space()=" + q + "]");
            }
            for (const a of ["name", "aria-label", "placeholder", "data-testid", "value", "title", "alt", "href"]) {
                const v = this.getAttribute(a);
                if (v && v.length <= 120) {
                    const q = xq(v);
                    if (q) cands.push("//" + tag + "[@" + a + "=" + q + "]");
                }
            }
            let pinned = null;
            for (const xp of cands) {
                const p = place(xp);
                if (p === -1) return JSON.stringify({ kind: "xpath", v: xp });
                if (p > 0 && !pinned) pinned = "(" + xp + ")[" + p + "]";
            }
            if (pinned) return JSON.stringify({ kind: "xpath", v: pinned });
            // Structural nth-of-type path, extended upward until unique —
            // survives reloads, not layout changes (the recorder's trade-off)
            let s = "", cur = this;
            while (cur && cur.nodeType === 1 && cur.tagName !== "HTML") {
                const par = cur.parentElement;
                let seg;
                if (cur.id && !generated(cur.id)) {
                    seg = "#" + esc(cur.id);
                } else {
                    seg = cur.tagName.toLowerCase();
                    if (par) {
                        const same = Array.prototype.filter.call(par.children, (c) => c.tagName === cur.tagName);
                        if (same.length > 1) seg += ":nth-of-type(" + (same.indexOf(cur) + 1) + ")";
                    }
                }
                s = seg + (s ? " > " + s : "");
                if (uniqCss(s)) return JSON.stringify({ kind: "css", v: s });
                cur = par;
            }
            return "null";
        }"##;
        let v = self
            .cdp(
                to,
                "Runtime.callFunctionOn",
                serde_json::json!({ "objectId": oid, "functionDeclaration": ANCHOR,
                                   "returnByValue": true }),
                timeout_ms,
            )
            .ok()?;
        let parsed: serde_json::Value = v
            .get("result")
            .and_then(|x| x.get("value"))
            .and_then(serde_json::Value::as_str)
            .and_then(|s| serde_json::from_str(s).ok())?;
        Some((
            parsed.get("kind")?.as_str()?.to_string(),
            parsed.get("v")?.as_str()?.to_string(),
        ))
    }

    /// Wait (in-page — see `__shikisha_ready` in the INIT script) until the
    /// element behind `oid` is actionable, and get its
    /// action point. Uses CDP's awaitPromise: the renderer resolves when the
    /// element settles, so the synchronization is the promise itself.
    /// Returns (ok, x, y, why)
    fn ref_ready(
        &self,
        to: Option<&str>,
        oid: &str,
        hit: bool,
        deadline_ms: u64,
        timeout_ms: u64,
    ) -> Result<(bool, f64, f64, String)> {
        const READY: &str = r#"function (deadline, hit) {
            return window.__shikisha_ready(this, { deadline: deadline, enabled: true, hit: hit })
                .then((r) => JSON.stringify(r));
        }"#;
        let v = self.cdp(
            to,
            "Runtime.callFunctionOn",
            serde_json::json!({ "objectId": oid, "functionDeclaration": READY,
                               "arguments": [{ "value": deadline_ms }, { "value": hit }],
                               "returnByValue": true, "awaitPromise": true }),
            timeout_ms,
        )?;
        let r: serde_json::Value = v
            .get("result")
            .and_then(|x| x.get("value"))
            .and_then(serde_json::Value::as_str)
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default();
        Ok((
            r.get("ok").and_then(serde_json::Value::as_bool).unwrap_or(false),
            r.get("x").and_then(serde_json::Value::as_f64).unwrap_or(0.0),
            r.get("y").and_then(serde_json::Value::as_f64).unwrap_or(0.0),
            r.get("why").and_then(serde_json::Value::as_str).unwrap_or("").to_string(),
        ))
    }

    /// The element's identity for the click echo (tag + visible text)
    fn click_desc(&self, to: Option<&str>, oid: &str, timeout_ms: u64) -> Option<String> {
        const DESC: &str = r#"function () {
            const t = this.innerText || this.value || this.getAttribute("aria-label")
                   || this.getAttribute("alt") || "";
            return this.tagName.toLowerCase() + " 「"
                 + Array.from(String(t).replace(/\s+/g, " ").trim()).slice(0, 60).join("") + "」";
        }"#;
        self.cdp(
            to,
            "Runtime.callFunctionOn",
            serde_json::json!({ "objectId": oid, "functionDeclaration": DESC,
                               "returnByValue": true }),
            timeout_ms,
        )
        .ok()
        .and_then(|v| {
            v.get("result")
                .and_then(|x| x.get("value"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
    }

    fn click_ref_inner(&self, to: Option<&str>, r: u32, timeout_ms: u64) -> Result<OpReport> {
        let oid = self.ref_object(to, r, timeout_ms)?;
        // Auto-wait for visible/enabled/stable and a clean hit target
        // (scrolling happens inside, cycling alignments per retry)
        let deadline = timeout_ms.saturating_sub(1_500).max(1_000);
        let (ok, x, y, why) = self
            .ref_ready(to, &oid, true, deadline, timeout_ms)
            .map_err(|e| Self::ref_stale(r, e))?;
        let desc = self.click_desc(to, &oid, timeout_ms);
        let anchor = self.element_anchor(to, &oid, timeout_ms);

        let synthetic_click = || {
            self.cdp(
                to,
                "Runtime.callFunctionOn",
                serde_json::json!({ "objectId": oid,
                                   "functionDeclaration": "function () { this.click(); return true; }",
                                   "returnByValue": true }),
                timeout_ms,
            )
            .map(|_| ())
        };

        if !ok {
            if why == "not_found" {
                return Err(anyhow!(shikisha_core::i18n::tp(
                    "err.browser.ref_stale",
                    &[("ref", &r.to_string()), ("e", "detached")]
                )));
            }
            // Never actionable within the deadline (covered / unstable /
            // hidden): honor the ref with the element's own click() — the
            // pre-auto-wait behavior — and record why
            shikisha_core::append_hook_log(&format!(
                "ref click {r}: not actionable ({why}) — using the element's own click()"
            ));
            synthetic_click()?;
            return Ok(OpReport { state: Found::Visible, echo: desc, anchor });
        }

        const ACK_MS: u64 = 1_500;
        let probe = self.cdp(
            to,
            "Input.dispatchMouseEvent",
            serde_json::json!({ "type": "mouseMoved", "x": x, "y": y,
                               "button": "left", "buttons": 0, "clickCount": 0 }),
            ACK_MS,
        );
        if probe.is_ok() {
            for (kind, buttons) in [("mousePressed", 1), ("mouseReleased", 0)] {
                self.cdp(
                    to,
                    "Input.dispatchMouseEvent",
                    serde_json::json!({ "type": kind, "x": x, "y": y,
                                       "button": "left", "buttons": buttons, "clickCount": 1 }),
                    timeout_ms,
                )?;
            }
            return Ok(OpReport { state: Found::Visible, echo: desc, anchor });
        }
        shikisha_core::append_hook_log(&format!(
            "ref click {r}: no input ack — falling back to synthetic click"
        ));
        synthetic_click()?;
        Ok(OpReport { state: Found::Visible, echo: desc, anchor })
    }

    /// Resolve a ref to a JS object handle (for focus/read, not for input)
    fn ref_object(&self, to: Option<&str>, r: u32, timeout_ms: u64) -> Result<String> {
        let b = self.ref_backend(to, r)?;
        let node = self
            .cdp(
                to,
                "DOM.resolveNode",
                serde_json::json!({ "backendNodeId": b }),
                timeout_ms,
            )
            .map_err(|e| Self::ref_stale(r, e))?;
        node.get("object")
            .and_then(|o| o.get("objectId"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| {
                anyhow!(shikisha_core::i18n::tp(
                    "err.browser.ref_stale",
                    &[("ref", &r.to_string()), ("e", "resolveNode")]
                ))
            })
    }

    /// Fill a digest ref: focus + select-all, then type the value as genuine
    /// per-character key events. The same path the phone relay uses — sites
    /// like Google that ignore synthetic input events accept these.
    /// Wrapped in a compositor wake like clicks: a hidden page swallows
    /// keystrokes too (focus never lands without it). Returns the state plus
    /// an echo of which field was written (never its value — it may be secret)
    fn fill_ref(&self, to: Option<&str>, r: u32, value: &str, timeout_ms: u64) -> Result<OpReport> {
        self.wake(to, true);
        let out = self.fill_ref_inner(to, r, value, timeout_ms);
        self.wake(to, false);
        out
    }

    /// The field's identity for the echo: tag plus its label-ish attribute.
    /// Deliberately attribute-only — the field's value never appears here
    fn field_desc(&self, to: Option<&str>, oid: &str, timeout_ms: u64) -> Option<String> {
        const DESC: &str = r#"function () {
            const t = this.getAttribute("placeholder") || this.getAttribute("aria-label")
                   || this.getAttribute("name") || this.id || "";
            return this.tagName.toLowerCase()
                 + (t ? " 「" + Array.from(String(t)).slice(0, 40).join("") + "」" : "");
        }"#;
        self.cdp(
            to,
            "Runtime.callFunctionOn",
            serde_json::json!({ "objectId": oid, "functionDeclaration": DESC,
                               "returnByValue": true }),
            timeout_ms,
        )
        .ok()
        .and_then(|v| {
            v.get("result")
                .and_then(|x| x.get("value"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
    }

    fn fill_ref_inner(&self, to: Option<&str>, r: u32, value: &str, timeout_ms: u64) -> Result<OpReport> {
        // Make the page believe it has focus even when its window doesn't
        // (hidden or unfocused pages otherwise drop keystrokes). Sticky per
        // session and harmless when visible, so arming is idempotent
        let _ = self.cdp(
            to,
            "Emulation.setFocusEmulationEnabled",
            serde_json::json!({ "enabled": true }),
            timeout_ms,
        );
        let oid = self.ref_object(to, r, timeout_ms)?;
        // Auto-wait for visible/enabled/stable (scrolls into view inside).
        // A field that never settles degrades to acting anyway — the value
        // write is verified afterwards either way
        let deadline = timeout_ms.saturating_sub(1_500).max(1_000);
        let (ok, _, _, why) = self
            .ref_ready(to, &oid, false, deadline, timeout_ms)
            .map_err(|e| Self::ref_stale(r, e))?;
        if !ok && why == "not_found" {
            return Err(anyhow!(shikisha_core::i18n::tp(
                "err.browser.ref_stale",
                &[("ref", &r.to_string()), ("e", "detached")]
            )));
        }
        // Select everything so the typed characters replace the current value
        const FOCUS_SELECT: &str = r#"function () {
            this.focus();
            if (typeof this.select === "function") {
                this.select();
            } else if (this.isContentEditable) {
                const r = document.createRange();
                r.selectNodeContents(this);
                const s = window.getSelection();
                s.removeAllRanges();
                s.addRange(r);
            }
            return true;
        }"#;
        self.cdp(
            to,
            "Runtime.callFunctionOn",
            serde_json::json!({ "objectId": oid, "functionDeclaration": FOCUS_SELECT,
                               "returnByValue": true }),
            timeout_ms,
        )?;
        // The framework-aware write __shikisha_fill also uses: the native
        // setter plus input/change events. Works regardless of visibility
        const SET_VALUE: &str = r#"function (v) {
            if (this.isContentEditable) {
                this.textContent = v;
            } else {
                const proto =
                    this instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype
                    : this instanceof HTMLSelectElement ? HTMLSelectElement.prototype
                    : HTMLInputElement.prototype;
                const d = Object.getOwnPropertyDescriptor(proto, "value");
                if (d && d.set) d.set.call(this, v); else this.value = v;
            }
            this.dispatchEvent(new Event("input", { bubbles: true }));
            this.dispatchEvent(new Event("change", { bubbles: true }));
            return true;
        }"#;
        let set_native = || {
            self.cdp(
                to,
                "Runtime.callFunctionOn",
                serde_json::json!({ "objectId": oid, "functionDeclaration": SET_VALUE,
                                   "arguments": [{ "value": value }],
                                   "returnByValue": true }),
                timeout_ms,
            )
            .map(|_| ())
        };
        let desc = self.field_desc(to, &oid, timeout_ms);
        let anchor = self.element_anchor(to, &oid, timeout_ms);
        if value.is_empty() {
            set_native()?;
            return Ok(OpReport { state: Found::Visible, echo: desc, anchor });
        }
        for ch in value.chars() {
            self.cdp(
                to,
                "Input.dispatchKeyEvent",
                serde_json::json!({ "type": "char", "text": ch.to_string() }),
                timeout_ms,
            )?;
        }
        // Keystrokes can be silently swallowed (a hidden page acks them but
        // inserts nothing, since focus never lands). Verify what's in the
        // field; if the typing didn't take, write through the native setter
        // so the fill never "succeeds" while the field stays empty
        const READ: &str = r#"function () {
            return this.value !== undefined ? String(this.value)
                 : (this.innerText || this.textContent || "");
        }"#;
        let got = self
            .cdp(
                to,
                "Runtime.callFunctionOn",
                serde_json::json!({ "objectId": oid, "functionDeclaration": READ,
                                   "returnByValue": true }),
                timeout_ms,
            )
            .ok()
            .and_then(|v| {
                v.get("result")
                    .and_then(|x| x.get("value"))
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            });
        if got.as_deref() != Some(value) {
            // Never log the value itself (it may be sensitive) — only the fact
            shikisha_core::append_hook_log(&format!(
                "ref fill {r}: keystrokes didn't land (page hidden?) — falling back to native setter"
            ));
            set_native()?;
        }
        Ok(OpReport { state: Found::Visible, echo: desc, anchor })
    }

    /// Where a digest ref currently is, in the same three-state vocabulary
    /// selectors use: gone = `not_found`, outside the viewport = `off_screen`
    fn find_ref(&self, to: Option<&str>, r: u32, timeout_ms: u64) -> Result<Found> {
        let b = self.ref_backend(to, r)?;
        let Ok(q) = self.cdp(
            to,
            "DOM.getContentQuads",
            serde_json::json!({ "backendNodeId": b }),
            timeout_ms,
        ) else {
            return Ok(Found::NotFound);
        };
        let Some((x, y)) = Self::quad_center(&q) else {
            return Ok(Found::NotFound);
        };
        let m = self.cdp(to, "Page.getLayoutMetrics", serde_json::json!({}), timeout_ms)?;
        let vp = m.get("cssVisualViewport");
        let w = vp.and_then(|v| v.get("clientWidth")).and_then(serde_json::Value::as_f64);
        let h = vp.and_then(|v| v.get("clientHeight")).and_then(serde_json::Value::as_f64);
        let on = match (w, h) {
            (Some(w), Some(h)) => x >= 0.0 && y >= 0.0 && x < w && y < h,
            _ => true,
        };
        Ok(if on { Found::Visible } else { Found::OffScreen })
    }

    /// Read a digest ref's text (an input's value, or the displayed string)
    fn text_ref(&self, to: Option<&str>, r: u32, timeout_ms: u64) -> Result<Option<String>> {
        let oid = self.ref_object(to, r, timeout_ms)?;
        const READ: &str = r#"function () {
            return this.value !== undefined ? String(this.value)
                 : (this.innerText || this.textContent || "");
        }"#;
        let v = self.cdp(
            to,
            "Runtime.callFunctionOn",
            serde_json::json!({ "objectId": oid, "functionDeclaration": READ,
                               "returnByValue": true }),
            timeout_ms,
        )?;
        Ok(v.get("result")
            .and_then(|x| x.get("value"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string))
    }
}

impl Drop for Browser {
    /// Don't leave behind a window whose conductor is gone.
    /// It's fine if closing fails (that just means the other side already died first)
    fn drop(&mut self) {
        let _ = self.proxy.send_event(Cmd::Close);
    }
}

/// Wrap an expression so its result comes back over IPC.
///
/// Wrapped in an async function and awaited, so an async value like the
/// result of `fetch` also gets resolved before returning. Awaiting a
/// synchronous value just passes it through, so existing DOM calls still work as-is
fn wrap_eval(id: u64, js: &str) -> String {
    format!(
        r#"(async function(){{
  try {{
    var v = await (async function(){{ {js} }})();
    window.ipc.postMessage(JSON.stringify({{kind:"result",id:{id},ok:true,
      value: v === undefined ? null : v}}));
  }} catch (e) {{
    window.ipc.postMessage(JSON.stringify({{kind:"result",id:{id},ok:false,
      value: String(e && e.message || e)}}));
  }}
}})();"#
    )
}







/// Resolve an instruction's destination. `None` is the main view; a name is that page.
/// If a name is given but not found, returns `None`.
/// Falling back to the main view would run site-facing JS against our own screen
/// Convert a control key name for the screencast view into what CDP needs (key name, Windows virtual key code)
fn named_vk(named: &str) -> Option<(&'static str, u32)> {
    Some(match named {
        "enter" => ("Enter", 13),
        "backspace" => ("Backspace", 8),
        "tab" => ("Tab", 9),
        "escape" | "esc" => ("Escape", 27),
        "delete" => ("Delete", 46),
        "up" => ("ArrowUp", 38),
        "down" => ("ArrowDown", 40),
        "left" => ("ArrowLeft", 37),
        "right" => ("ArrowRight", 39),
        "space" => (" ", 32),
        "home" => ("Home", 36),
        "end" => ("End", 35),
        "pageup" => ("PageUp", 33),
        "pagedown" => ("PageDown", 34),
        "f1" => ("F1", 112),
        "f2" => ("F2", 113),
        "f3" => ("F3", 114),
        "f4" => ("F4", 115),
        "f5" => ("F5", 116),
        "f6" => ("F6", 117),
        "f7" => ("F7", 118),
        "f8" => ("F8", 119),
        "f9" => ("F9", 120),
        "f10" => ("F10", 121),
        "f11" => ("F11", 122),
        "f12" => ("F12", 123),
        _ => return None,
    })
}

/// Whether `named` is a control key we can dispatch (enter/tab/escape/arrows/
/// f-keys/…). Lets `browser_press` reject a typo instead of silently no-op-ing.
pub fn key_known(named: &str) -> bool {
    // The list is shared vocabulary; this module only knows what to *do* with a
    // name. Debug-checked so the two cannot drift apart unnoticed
    debug_assert_eq!(
        shikisha_shared::key_known(named),
        named_vk(named).is_some(),
        "key name known to one side and not the other: {named}"
    );
    shikisha_shared::key_known(named)
}

/// Windows the pages asked for, by the page that asked. Newest last.
///
/// A pane holds one page at a time, so a window opened by one of them stands
/// in the same seat, in front of it. Which is also what it means to everything
/// else: "this pane" is what a person is looking at and what automation is
/// driving, and while a sign-in window is up, that is the sign-in window.
type Overlays = std::collections::HashMap<String, Vec<(String, wry::WebView)>>;

/// The board's page, while there is one (none while the window is put away)
fn main_view(shell: &Option<(wry::WebContext, wry::WebView)>) -> Option<&wry::WebView> {
    shell.as_ref().map(|(_, view)| view)
}

fn target<'a>(
    main: Option<&'a wry::WebView>,
    children: &'a std::collections::HashMap<String, wry::WebView>,
    overlays: &'a Overlays,
    to: &Option<String>,
) -> Option<&'a wry::WebView> {
    match to {
        None => main,
        Some(name) => overlays
            .get(name)
            .and_then(|stack| stack.last())
            .map(|(_, v)| v)
            .or_else(|| children.get(name)),
    }
}

/// What a new-window handler has to hand back to the loop it cannot touch.
#[derive(Default)]
struct Adoptions {
    /// (the page that asked, the new page's name, the page itself)
    made: Vec<(String, String, wry::WebView)>,
    /// Names of adopted pages that asked to be let go
    shut: Vec<String>,
    /// Counts the names apart
    next: u32,
}

/// One command line for the whole answer, because the answer has to be given
/// synchronously and this is the only place it can be built.
type WindowAnswer = Box<dyn Fn(String, wry::NewWindowFeatures) -> wry::NewWindowResponse>;

/// Everything a browser has to say about who it is, once a name is chosen.
///
/// The name is not the only place the claim is made. It is made again, in
/// `Sec-CH-UA` and in `navigator.userAgentData`, and those are not written
/// from the name — a browser calling itself Chrome while announcing
/// "Microsoft Edge WebView2" alongside has told a site more than it would
/// have by saying nothing at all. So the brands are built out of the name
/// itself, and the two always agree.
///
/// A name with no Chromium version in it (someone naming a browser that is
/// not one) is given no brands: that is what a browser which does not speak
/// client hints sends.
fn ua_override(ua: &str) -> String {
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
            Some(edge) => {
                brands.push(serde_json::json!({ "brand": "Microsoft Edge", "version": edge }))
            }
            None => brands
                .push(serde_json::json!({ "brand": "Google Chrome", "version": major })),
        }
    }
    serde_json::json!({
        "userAgent": ua,
        "userAgentMetadata": {
            "brands": brands,
            "platform": "Windows",
            "platformVersion": "15.0.0",
            "architecture": "x86",
            "bitness": "64",
            "model": "",
            "mobile": false,
        },
    })
    .to_string()
}

/// Take in a window a page asked for, rather than refusing it in silence.
///
/// wry's answer to `window.open` when nobody says otherwise is "no", with
/// nothing said: the call returns null, the `target="_blank"` link does
/// nothing, and a sign-in button that hands off to a popup — which is most of
/// them — is a button that does nothing at all. There is no error for anyone
/// to read, on either side.
///
/// So the window is made here and handed back, which is what keeps the two
/// pages related: same environment, real opener, so `window.opener`,
/// `postMessage` and the closing handshake all still work. It is placed in
/// the seat its opener occupies, because a pane is where a person is looking.
fn adopt_windows(
    opener: String,
    seat: std::rc::Rc<std::cell::Cell<(i32, i32, i32, i32)>>,
    window: std::rc::Rc<tao::window::Window>,
    inbox: std::rc::Rc<std::cell::RefCell<Adoptions>>,
    proxy: tao::event_loop::EventLoopProxy<Cmd>,
    ev_tx: Sender<Ev>,
    // A sign-in window that called itself something else than the page that
    // opened it would be a second browser arriving in the middle of a login
    user_agent: Option<String>,
) -> WindowAnswer {
    use wry::{NewWindowResponse, WebViewBuilder, WebViewBuilderExtWindows};
    Box::new(move |uri, features| {
        let name = {
            let mut ib = match inbox.try_borrow_mut() {
                Ok(ib) => ib,
                // Never make a window while the loop is in the middle of
                // counting them. Refusing is what happens today anyway
                Err(_) => return NewWindowResponse::Deny,
            };
            ib.next += 1;
            format!("{opener}#window{}", ib.next)
        };
        // No URL is given: the runtime navigates the window it is handed, and
        // setting one here would load the page twice
        let ipc_name = name.clone();
        let ipc_inbox = std::rc::Rc::clone(&inbox);
        let ipc_proxy = proxy.clone();
        let nav_tx = ev_tx.clone();
        let nav_who = opener.clone();
        let fin_tx = ev_tx.clone();
        let fin_who = opener.clone();
        let mut b = WebViewBuilder::new();
        if let Some(ua) = user_agent.as_deref() {
            b = b.with_user_agent(ua);
        }
        let built = b
            .with_environment(features.opener.environment.clone())
            .with_bounds(to_rect(seat.get()))
            .with_initialization_script(&format!("{INIT_JS}{PLACED_JS}{POPUP_JS}"))
            // Reported as the pane, not as itself: what the bar above the pane
            // should say is loading is whatever the pane is showing
            .with_navigation_handler(move |_url| {
                let _ = nav_tx.send(Ev::Loading { from: Some(nav_who.clone()), busy: true });
                true
            })
            .with_on_page_load_handler(move |e, _url| {
                if matches!(e, wry::PageLoadEvent::Finished) {
                    let _ = fin_tx.send(Ev::Loading { from: Some(fin_who.clone()), busy: false });
                }
            })
            .with_ipc_handler(move |req| {
                let body: &str = req.body();
                if !body.contains("popupclose") {
                    return;
                }
                if let Ok(mut ib) = ipc_inbox.try_borrow_mut() {
                    ib.shut.push(ipc_name.clone());
                    let _ = ipc_proxy.send_event(Cmd::Adopt);
                }
            })
            // A window opened by a window belongs to the same seat
            .with_new_window_req_handler(adopt_windows(
                opener.clone(),
                std::rc::Rc::clone(&seat),
                std::rc::Rc::clone(&window),
                std::rc::Rc::clone(&inbox),
                proxy.clone(),
                ev_tx.clone(),
                user_agent.clone(),
            ))
            .build_as_child(&*window);
        match built {
            Ok(v) => {
                let raw = cdp::webview_of(&v);
                if let Some(ua) = user_agent.as_deref() {
                    cdp::call(&raw, "Emulation.setUserAgentOverride", &ua_override(ua));
                }
                shikisha_core::append_hook_log(&format!(
                    "[browser] '{opener}' opened a window -> '{name}' ({uri})"
                ));
                if let Ok(mut ib) = inbox.try_borrow_mut() {
                    ib.made.push((opener.clone(), name, v));
                }
                let _ = proxy.send_event(Cmd::Adopt);
                NewWindowResponse::Create { webview: raw }
            }
            Err(e) => {
                shikisha_core::append_hook_log(&format!(
                    "[browser] '{opener}' asked for a window ({uri}) and it could not be made: {e}"
                ));
                NewWindowResponse::Deny
            }
        }
    })
}

/// Build a JS function call.
///
/// **Arguments must always go through here.** Everything is serialized
/// with `serde_json`, so quotes and newlines survive intact and the value
/// passed in is never interpreted as code. Even AI output or text read
/// straight off a page arrives as a plain value
fn call_js(func: &str, args: &[serde_json::Value]) -> String {
    let list: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    format!("return window.{func}({});", list.join(","))
}

/// Convert a position and size into wry's shape
fn to_rect((x, y, w, h): (i32, i32, i32, i32)) -> wry::Rect {
    wry::Rect {
        position: wry::dpi::LogicalPosition::new(x, y).into(),
        size: wry::dpi::LogicalSize::new(w.max(0), h.max(0)).into(),
    }
}

/// Turn text a human typed into a destination we're allowed to open.
///
/// Works like a browser's combined address/search box: text that reads as a
/// web address goes there (`example.com` -> `https://example.com`), and
/// anything else — words with spaces, Japanese text, a lone word — becomes a
/// Google search. `file:` can read local files and `javascript:` can hijack
/// the current page, so neither passes through an address bar — a "gateway
/// to anywhere"; they too fall through to search, which is inert.
pub fn openable(raw: &str) -> Option<String> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    if let Some((scheme, rest)) = s.split_once("://") {
        // An explicit scheme means the writer wanted a URL, not a search.
        // Normalize its case so a pasted HTTPS:// still opens.
        if scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https") {
            return Some(format!("{}://{rest}", scheme.to_ascii_lowercase()));
        }
        // file:// and friends never open here — hand them to search instead
        return Some(search_url(s));
    }
    // Scheme-less: a single token whose host part has a dot (example.com,
    // 127.0.0.1) or is localhost reads as an address; everything else —
    // including `javascript:alert(1)`, which has no dot — reads as words
    let host = s.split(['/', '?', '#']).next().unwrap_or("");
    let address_like = !s.chars().any(char::is_whitespace)
        && (host.contains('.') || host == "localhost" || host.starts_with("localhost:"));
    if address_like {
        Some(format!("https://{s}"))
    } else {
        Some(search_url(s))
    }
}

/// A Google search for the given words, with every byte outside the URL-safe
/// set percent-encoded (UTF-8), so Japanese and symbols survive the trip
fn search_url(words: &str) -> String {
    use std::fmt::Write as _;
    let mut u = String::from("https://www.google.com/search?q=");
    for b in words.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                u.push(*b as char)
            }
            b' ' => u.push('+'),
            _ => {
                let _ = write!(u, "%{b:02X}");
            }
        }
    }
    u
}

/// Put our own icon on the window.
///
/// A window that never says which icon it wants gets Windows' default — the
/// grey generic window shape — in its title bar and in Alt+Tab, while the
/// taskbar goes and finds the one inside the exe. Two pictures for one program,
/// and the one in the corner of our own window was not even ours.
///
/// It is loaded out of our own resource (id 1, which is where `build.rs` puts
/// `assets\icon.ico`) rather than from a file beside the exe: one copy of the
/// artwork, nothing extra to ship, and nothing that can go missing.
///
/// Each size is asked for by name. An icon file holds several drawings, and
/// `LoadImage` picks the one nearest what it is asked for; asking for
/// "whatever" (0, 0) takes the first entry and squeezes it, which is how a
/// 256-pixel drawing ends up as a smear in a 16-pixel corner.
#[cfg(windows)]
/// The window's handle, for the few things that must be told which window
/// they belong to (the Store's update dialog)
static MAIN_HWND: std::sync::OnceLock<isize> = std::sync::OnceLock::new();

pub fn main_hwnd() -> isize {
    MAIN_HWND.get().copied().unwrap_or(0)
}

fn wear_our_own_icon(hwnd: isize) {
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, ICON_BIG, ICON_SMALL, IMAGE_ICON, LR_DEFAULTCOLOR, LoadImageW,
        SM_CXICON, SM_CXSMICON, SM_CYICON, SM_CYSMICON, SendMessageW, WM_SETICON,
    };
    const OUR_ICON: *const u16 = 1 as *const u16; // MAKEINTRESOURCE(1)
    unsafe {
        let module = GetModuleHandleW(std::ptr::null());
        let hwnd = hwnd as *mut std::ffi::c_void;
        let mut wear = |which: u32, w: i32, h: i32| {
            let icon = LoadImageW(module, OUR_ICON, IMAGE_ICON, w, h, LR_DEFAULTCOLOR);
            if !icon.is_null() {
                SendMessageW(hwnd, WM_SETICON, which as usize, icon as isize);
            }
        };
        // The title bar and Alt+Tab, then the taskbar and the switcher's big
        // tile. The system is asked how large those are, because it is not the
        // same answer on a 150% display as on a 100% one
        wear(
            ICON_SMALL,
            GetSystemMetrics(SM_CXSMICON),
            GetSystemMetrics(SM_CYSMICON),
        );
        wear(
            ICON_BIG,
            GetSystemMetrics(SM_CXICON),
            GetSystemMetrics(SM_CYICON),
        );
    }
}

fn run_window(
    url: &str,
    title: &str,
    proxy_tx: Sender<tao::event_loop::EventLoopProxy<Cmd>>,
    ev_tx: Sender<Ev>,
) -> Result<()> {
    use tao::event::{Event, WindowEvent};
    use tao::event_loop::{ControlFlow, EventLoopBuilder};
    use tao::platform::run_return::EventLoopExtRunReturn;
    use tao::platform::windows::EventLoopBuilderExtWindows;
    use tao::window::WindowBuilder;
    use wry::{WebContext, WebViewBuilder};

    // Runs on a separate thread from the TUI's render loop, so lift the main-thread restriction
    let mut ev_loop = EventLoopBuilder::<Cmd>::with_user_event()
        .with_any_thread(true)
        .build();
    proxy_tx
        .send(ev_loop.create_proxy())
        .map_err(|_| anyhow!(shikisha_core::i18n::t("err.browser.proxy_connect_failed")))?;

    // Shared, because a page asking to open a window is answered on the
    // message loop, and the answer is a page built inside this same window
    let window = std::rc::Rc::new(
        WindowBuilder::new()
            .with_title(title)
            .with_inner_size(tao::dpi::LogicalSize::new(1280.0, 900.0))
            .build(&ev_loop)?,
    );
    // The notification-area icon. Its presses, and a second copy's request
    // for the window, arrive through the window's own procedure (see tray.rs)
    let tray = {
        use tao::platform::windows::WindowExtWindows;
        let tell = ev_tx.clone();
        crate::tray::Tray::add(
            window.hwnd(),
            title,
            move |pressed| {
                let _ = match pressed {
                    crate::tray::Pressed::Open => tell.send(Ev::TrayOpen),
                    crate::tray::Pressed::Quit => tell.send(Ev::TrayQuit),
                    crate::tray::Pressed::Nothing => Ok(()),
                };
            },
            &shikisha_core::i18n::t("tray.open"),
            &shikisha_core::i18n::t("tray.quit"),
        )
    };
    #[cfg(windows)]
    {
        use tao::platform::windows::WindowExtWindows;
        wear_our_own_icon(window.hwnd());
        let _ = MAIN_HWND.set(window.hwnd());
    }

    // The board's own page. Built here, and built again whenever the window
    // comes back from being put away: while it is away the page is dropped
    // altogether, because the page is where the memory is. Chromium keeps a
    // browser process and a renderer for it -- some two hundred megabytes
    // for an empty board -- and hiding the window releases none of that.
    // The pages placed inside the window (a browser a rally is driving, the
    // settings) are kept: those hold state a person would lose.
    //
    // The shell gets an explicit folder for the same reason the pages do: without
    // one, WebView2 writes "<exe>.WebView2" next to the binary — into the folder
    // that is meant to hold nothing but the exe
    //
    // Every message a page posts is judged by the address it was posted from
    // (WebView2 reports it with the message; it is not something the page
    // writes). The board's own address is the app's, and only a page speaking
    // from one of the app's addresses is heard in full — see `heard`. The
    // settings server's address joins the list when it starts (Cmd::Trust)
    let own = std::rc::Rc::new(std::cell::RefCell::new(vec![url.to_string()]));
    // The questions put to pages and not yet answered, shared with every
    // page's handler: an answer is taken only from the page that was asked
    let asked = std::rc::Rc::new(std::cell::RefCell::new(Asked::default()));
    let shell_of = {
        let window = std::rc::Rc::clone(&window);
        let ev_tx = ev_tx.clone();
        let url = url.to_string();
        let own = std::rc::Rc::clone(&own);
        let asked = std::rc::Rc::clone(&asked);
        move || -> Result<(WebContext, wry::WebView)> {
            let ipc = ev_tx.clone();
            let own = std::rc::Rc::clone(&own);
            let asked = std::rc::Rc::clone(&asked);
            let refused = std::cell::Cell::new(0u8);
            let mut ctx = WebContext::new(Some(shell_data_dir()));
            let view = WebViewBuilder::new_with_web_context(&mut ctx)
                .with_url(&url)
                .with_initialization_script(INIT_JS)
                .with_ipc_handler(move |req| {
                    let at = req.uri().to_string();
                    let body: &str = req.body();
                    // The board is the app's own page. Were it ever led to
                    // another site, that site would hold the whole keyboard;
                    // so a board speaking from anywhere else is not the board
                    if !from_ours(&own.borrow(), &at) {
                        note_refused(&refused, None, &at, body);
                        return;
                    }
                    let ev = heard(body, None, true, &mut asked.borrow_mut());
                    if let Some(ev) = ev {
                        let _ = ipc.send(ev);
                    }
                })
                .build(&*window)?;
            Ok((ctx, view))
        }
    };
    let mut shell: Option<(WebContext, wry::WebView)> = Some(shell_of()?);

    // Pages placed inside the same window. Looked up by name
    let mut children: std::collections::HashMap<String, wry::WebView> =
        std::collections::HashMap::new();
    // What the browser calls itself. Read once, here: a name that changed
    // under a page would be a different browser halfway through a login
    let user_agent = shikisha_core::config::user_agent();
    // Windows those pages asked to open, kept in the seat of whoever asked
    let mut overlays: Overlays = std::collections::HashMap::new();
    // How those handlers hand their work back to this loop
    let adoptions = std::rc::Rc::new(std::cell::RefCell::new(Adoptions::default()));
    // ...and how they wake it to come and collect
    let adopt_wake = ev_loop.create_proxy();
    // WebContext per profile. One per data folder (tabs with the same name share it).
    // Not needed after creation on Windows, but keeping it around is harmless, so hold it keyed by folder
    let mut web_ctxs: std::collections::HashMap<std::path::PathBuf, WebContext> =
        std::collections::HashMap::new();
    // Temp folders for children placed in private mode (child name -> folder). Removed on close
    let mut ephemeral_dirs: std::collections::HashMap<String, std::path::PathBuf> =
        std::collections::HashMap::new();

    // Screencasts. One per target. Frames only arrive while this is held
    let mut casts: std::collections::HashMap<Option<String>, cdp::Cast> =
        std::collections::HashMap::new();
    // Compositor wakes for genuine input on hidden pages. A page hidden the
    // normal way (bounds 0×0) has no surface, and mouse input needs one to
    // hit-test against — its ack never comes. For the duration of a wake the
    // page gets a real-sized surface parked outside the client area (never
    // painted, so nothing flickers), plus a tiny throwaway screencast to
    // keep frames flowing. The bool remembers whether the bounds were
    // borrowed, so release restores exactly the layout's last word. Kept
    // separate from `casts` so a phone relay and a wake never fight
    let mut wakes: std::collections::HashMap<Option<String>, (cdp::Cast, bool)> =
        std::collections::HashMap::new();
    // Each child's last layout-given rect: how a wake tells hidden (0×0)
    // from merely unfocused, and what it restores on release
    // Shared per child: a handler that opens a window into this seat reads it
    // without reaching into the loop's own state, and a resize lands on the
    // window that is standing in the seat as well
    let mut child_sizes: std::collections::HashMap<
        String,
        std::rc::Rc<std::cell::Cell<(i32, i32, i32, i32)>>,
    > = std::collections::HashMap::new();
    // Each cast target's own (pre-override) viewport in CSS px. Held for the
    // life of the cast so a phone rotating back and forth always re-shapes
    // from the real size, and torn down (override cleared) with the cast
    let mut naturals: std::collections::HashMap<Option<String>, (f64, f64)> =
        std::collections::HashMap::new();
    // Basic-auth arming. One per target. Only answers 401s while this is held
    let mut auths: std::collections::HashMap<Option<String>, cdp::AuthArm> =
        std::collections::HashMap::new();
    // Automatic handling of JS dialogs. One per child. Without this, automation freezes on things like "leave this page?" confirmations
    let mut dialogs: std::collections::HashMap<Option<String>, cdp::DialogArm> =
        std::collections::HashMap::new();
    // The most recent frame's CSS pixel dimensions (used to convert
    // coordinates for input injection).
    // Frame notification and input injection run on the same thread, so `Rc<Cell>` is enough
    let cast_dims = std::rc::Rc::new(std::cell::Cell::new((0.0f64, 0.0f64)));
    // For drag detection: is the button currently held down
    let mut mouse_down = false;

    // Reports are sent from inside the loop too, so grab a sender for "closed" ahead of time
    let closed_tx = ev_tx.clone();
    // The channel that answers "where are we now". Only known from inside the window, so it answers from here
    let where_tx = ev_tx.clone();
    ev_loop.run_return(move |event, _, control| {
        *control = ControlFlow::Wait;
        match event {
            Event::UserEvent(cmd) => match cmd {
                Cmd::Trust { origin } => {
                    let mut list = own.borrow_mut();
                    if !list.iter().any(|o| same_origin(o, &origin)) {
                        list.push(origin);
                    }
                }
                Cmd::Eval { id, to, js } => {
                    // When the destination can't be found, don't fall back
                    // to the main view. That would run site-facing JS against our own screen
                    if let Some(v) = target(main_view(&shell), &children, &overlays, &to) {
                        // Written down before the page is asked, so the answer
                        // is expected — from this page — by the time it comes
                        asked.borrow_mut().ask(id, to.clone());
                        let _ = v.evaluate_script(&wrap_eval(id, &js));
                    } else {
                        let _ = ev_tx.send(Ev::Result {
                            id,
                            ok: false,
                            value: serde_json::Value::String(shikisha_core::i18n::tp(
                                "err.browser.page_not_placed",
                                &[("to", &to.unwrap_or_default())],
                            ))
                            .to_string(),
                        });
                    }
                }
                Cmd::Wake { to, on } => {
                    if on {
                        // A live relay cast already keeps the compositor
                        // running; arming a second screencast would replace
                        // its parameters and stopping it later would kill
                        // the relay's frames. So wake only when nothing casts
                        if !wakes.contains_key(&to) && !casts.contains_key(&to) {
                            if let Some(v) = target(main_view(&shell), &children, &overlays, &to) {
                                let wv = cdp::webview_of(v);
                                // Hidden = this child is currently sized 0.
                                // Borrow it a real surface, parked outside
                                // the client area (clipped, never painted)
                                let hidden = to.as_ref().is_some_and(|name| {
                                    child_sizes.get(name).is_none_or(|seat| {
                                        let (_, _, w, h) = seat.get();
                                        w <= 0 || h <= 0
                                    })
                                });
                                if hidden {
                                    let _ = v.set_bounds(to_rect((-4000, 0, 1280, 900)));
                                }
                                if let Some(cast) =
                                    cdp::start_with(&wv, cdp::WAKE_PARAMS, |_, _, _| {})
                                {
                                    wakes.insert(to.clone(), (cast, hidden));
                                } else if hidden {
                                    if let Some(r) = to
                                        .as_ref()
                                        .and_then(|n| child_sizes.get(n))
                                        .map(|seat| seat.get())
                                    {
                                        let _ = v.set_bounds(to_rect(r));
                                    }
                                }
                            }
                        }
                    } else if let Some((cast, borrowed)) = wakes.remove(&to) {
                        cdp::stop(cast);
                        if borrowed {
                            // Give back whatever the layout last decreed —
                            // including a rect that changed mid-wake
                            if let (Some(v), Some(r)) = (
                                target(main_view(&shell), &children, &overlays, &to),
                                to.as_ref()
                                    .and_then(|n| child_sizes.get(n))
                                    .map(|seat| seat.get()),
                            ) {
                                let _ = v.set_bounds(to_rect(r));
                            }
                        }
                    }
                }
                Cmd::Cdp { id, to, method, params } => {
                    // Same guard as Eval: an unplaced page must answer, not hang
                    if let Some(v) = target(main_view(&shell), &children, &overlays, &to) {
                        let tx = ev_tx.clone();
                        cdp::call_result(
                            &cdp::webview_of(v),
                            &method,
                            &params,
                            move |ok, json| {
                                let _ = tx.send(Ev::Result { id, ok, value: json });
                            },
                        );
                    } else {
                        let _ = ev_tx.send(Ev::Result {
                            id,
                            ok: false,
                            value: shikisha_core::i18n::tp(
                                "err.browser.page_not_placed",
                                &[("to", &to.unwrap_or_default())],
                            ),
                        });
                    }
                }
                Cmd::BasicAuth { to, user, pass } => {
                    // If credentials are already armed, swap them; otherwise enable Fetch and arm them
                    if let Some(arm) = auths.get(&to) {
                        *arm.creds.borrow_mut() = (user, pass);
                    } else if let Some(v) = target(main_view(&shell), &children, &overlays, &to) {
                        let wv = cdp::webview_of(v);
                        match cdp::arm_basic_auth(&wv, &user, &pass) {
                            Some(arm) => {
                                auths.insert(to.clone(), arm);
                            }
                            None => shikisha_core::append_hook_log(&shikisha_core::i18n::t(
                                "err.browser.log_basic_auth_failed",
                            )),
                        }
                    }
                }
                Cmd::AddChild { name, url, rect, profile } => {
                    // Creating a WebView2 controller runs synchronously ON THIS
                    // event-loop thread, and this thread also pumps the whole
                    // window's messages — while it runs, every click is frozen.
                    // Log how long it took so a "dead window right after
                    // startup" report can be matched against it.
                    let born = std::time::Instant::now();
                    let bounds = to_rect(rect);
                    let seat = std::rc::Rc::new(std::cell::Cell::new(rect));
                    child_sizes.insert(name.clone(), std::rc::Rc::clone(&seat));
                    // Decide this page's data storage (profile/private).
                    // Same folder = same cookies/login, different folder = different profile.
                    // All tabs, including "default", are isolated under
                    // browser-profiles/<name> (like Chrome's "person").
                    // Private mode gets a unique temp folder on every call
                    // and is removed on close.
                    let data_dir = profile_dir(&profile);
                    if profile.private {
                        ephemeral_dirs.insert(name.clone(), data_dir.clone());
                    }
                    // This page's own name if it was given one, the app's
                    // otherwise
                    let ua = profile.user_agent.clone().or_else(|| user_agent.clone());
                    let ctx = web_ctxs
                        .entry(data_dir.clone())
                        .or_insert_with(|| WebContext::new(Some(data_dir.clone())));
                    // Equip the child with the same tools as the main view.
                    // Without them, a placed page would just be something displayed, nothing more
                    let ipc = ev_tx.clone();
                    let who = name.clone();
                    let ipc_own = std::rc::Rc::clone(&own);
                    let ipc_asked = std::rc::Rc::clone(&asked);
                    let refused = std::cell::Cell::new(0u8);
                    // Signaling "in progress" from the in-page script (at
                    // document creation) is too late. If the server is slow,
                    // the document isn't created until the response comes
                    // back, so the indicator would stay off the whole time
                    // we're waiting. Instead, turn it on when navigation
                    // starts (the moment it's pressed, before any response)
                    // and off when loading finishes. This keeps it lit for the entire wait
                    let nav_tx = ev_tx.clone();
                    let nav_who = name.clone();
                    let fin_tx = ev_tx.clone();
                    let fin_who = name.clone();
                    let mut b = WebViewBuilder::new_with_web_context(ctx);
                    if let Some(ua) = ua.as_deref() {
                        b = b.with_user_agent(ua);
                    }
                    match b
                        .with_url(&url)
                        .with_bounds(bounds)
                        .with_initialization_script(&format!("{INIT_JS}{PLACED_JS}"))
                        .with_navigation_handler(move |_url| {
                            let _ = nav_tx.send(Ev::Loading { from: Some(nav_who.clone()), busy: true });
                            true // Don't block the navigation. This is only here to emit a signal
                        })
                        .with_on_page_load_handler(move |e, _url| {
                            if matches!(e, wry::PageLoadEvent::Finished) {
                                let _ = fin_tx.send(Ev::Loading { from: Some(fin_who.clone()), busy: false });
                            }
                        })
                        // What to do when this page asks to open a window.
                        // Without an answer here the request is refused in
                        // silence, which is how a sign-in popup becomes a
                        // button that does nothing
                        .with_new_window_req_handler(adopt_windows(
                            name.clone(),
                            std::rc::Rc::clone(&seat),
                            std::rc::Rc::clone(&window),
                            std::rc::Rc::clone(&adoptions),
                            adopt_wake.clone(),
                            ev_tx.clone(),
                            ua.clone(),
                        ))
                        .with_ipc_handler(move |req| {
                            let at = req.uri().to_string();
                            let body: &str = req.body();
                            // The app's own pages (settings, a result view) are
                            // served from the board's address and keep their
                            // full voice. Anything else in this pane is
                            // somebody's website: it may report, not ask
                            let ours = from_ours(&ipc_own.borrow(), &at);
                            // There's no way to know who pressed it except here
                            let ev = heard(body, Some(&who), ours, &mut ipc_asked.borrow_mut());
                            match ev {
                                Some(ev) => {
                                    let _ = ipc.send(ev);
                                }
                                None => note_refused(&refused, Some(&who), &at, body),
                            }
                        })
                        .build_as_child(&*window)
                    {
                        Ok(v) => {
                            // Arm automatic dialog handling right after placing it (don't let "leave page?" freeze it)
                            let wvh = cdp::webview_of(&v);
                            // ...and say the same thing about who we are in
                            // the other place it gets asked
                            if let Some(ua) = ua.as_deref() {
                                cdp::call(&wvh, "Emulation.setUserAgentOverride", &ua_override(ua));
                            }
                            if let Some(arm) = cdp::arm_dialogs(&wvh) {
                                dialogs.insert(Some(name.clone()), arm);
                            }
                            shikisha_core::append_hook_log(&format!(
                                "[browser] placed page '{}' in {} ms (window input is frozen while a page is being created)",
                                name,
                                born.elapsed().as_millis()
                            ));
                            children.insert(name, v);
                        }
                        Err(e) => shikisha_core::append_hook_log(&shikisha_core::i18n::tp(
                            "err.browser.log_place_failed",
                            &[("name", &name), ("e", &format!("{e}"))],
                        )),
                    }
                }
                Cmd::ChildBounds { name, rect } => {
                    if let Some(v) = children.get(&name) {
                        let _ = v.set_bounds(to_rect(rect));
                        match child_sizes.get(&name) {
                            Some(seat) => seat.set(rect),
                            None => {
                                child_sizes.insert(
                                    name.clone(),
                                    std::rc::Rc::new(std::cell::Cell::new(rect)),
                                );
                            }
                        }
                        // A window this page opened stands in the same seat,
                        // so it is moved and hidden by the same layout
                        for (_, v) in overlays.get(&name).into_iter().flatten() {
                            let _ = v.set_bounds(to_rect(rect));
                        }
                    }
                }
                // Windows that were asked for, and windows that asked to go.
                // Both are handed over by handlers running on this same loop,
                // which is why neither can be done where it is decided
                Cmd::Adopt => {
                    let (made, shut) = match adoptions.try_borrow_mut() {
                        Ok(mut ib) => (std::mem::take(&mut ib.made), std::mem::take(&mut ib.shut)),
                        Err(_) => (Vec::new(), Vec::new()),
                    };
                    for (opener, name, view) in made {
                        overlays.entry(opener).or_default().push((name, view));
                    }
                    for name in shut {
                        // Dropping the page is what closes it
                        for stack in overlays.values_mut() {
                            stack.retain(|(n, _)| n != &name);
                        }
                        overlays.retain(|_, stack| !stack.is_empty());
                    }
                }
                Cmd::RemoveChild { name } => {
                    children.remove(&name);
                    child_sizes.remove(&name);
                    // Whatever it opened goes with it
                    overlays.remove(&name);
                    if let Some((cast, _)) = wakes.remove(&Some(name.clone())) {
                        cdp::stop(cast);
                    }
                    dialogs.remove(&Some(name.clone()));
                    auths.remove(&Some(name.clone()));
                    // If this child was placed in private mode, clean up
                    // its throwaway folder. WebView2 can take a moment to
                    // release the lock, so this is best-effort
                    // (anything missed gets swept up by sweep_private at startup)
                    if let Some(dir) = ephemeral_dirs.remove(&name) {
                        web_ctxs.remove(&dir);
                        erase_when_released(dir);
                    }
                }
                Cmd::Focus { to } => {
                    if let Some(v) = target(main_view(&shell), &children, &overlays, &to) {
                        if let Err(e) = v.focus() {
                            shikisha_core::append_hook_log(&shikisha_core::i18n::tp(
                                "err.browser.log_focus_failed",
                                &[("to", &format!("{to:?}")), ("e", &format!("{e}"))],
                            ));
                        }
                    }
                }
                Cmd::Move { to, go } => match target(main_view(&shell), &children, &overlays, &to) {
                    Some(v) => {
                        let r = match &go {
                            Go::Back => v.go_back(),
                            Go::Forward => v.go_forward(),
                            Go::Reload => v.reload(),
                            // wry's reload is the ordinary one. The flag that
                            // says "ignore what you already have" exists only
                            // in the DevTools protocol, so that is where this
                            // one goes
                            Go::Hard => {
                                cdp::call(
                                    &cdp::webview_of(v),
                                    "Page.reload",
                                    "{\"ignoreCache\":true}",
                                );
                                Ok(())
                            }
                            Go::To(u) => v.load_url(u),
                        };
                        if let Err(e) = r {
                            shikisha_core::append_hook_log(&shikisha_core::i18n::tp(
                                "err.browser.log_move_failed",
                                &[("go", &format!("{go:?}")), ("e", &format!("{e}"))],
                            ));
                        }
                    }
                    None => shikisha_core::append_hook_log(&shikisha_core::i18n::tp(
                        "err.browser.log_no_target",
                        &[("to", &format!("{to:?}"))],
                    )),
                },
                Cmd::Where { to } => {
                    if let Some(v) = target(main_view(&shell), &children, &overlays, &to) {
                        let _ = where_tx.send(Ev::Where {
                            from: to,
                            url: v.url().unwrap_or_default(),
                            can_back: v.can_go_back().unwrap_or(false),
                            can_forward: v.can_go_forward().unwrap_or(false),
                        });
                    }
                }
                Cmd::Screencast { to, on } => {
                    if on {
                        if casts.contains_key(&to) {
                            // Already streaming. We won't register twice,
                            // but re-issue startScreencast to push out one
                            // fresh frame (otherwise a new viewer joining
                            // while the page is static would see nothing indefinitely)
                            if let Some(view) = target(main_view(&shell), &children, &overlays, &to) {
                                cdp::kick(&cdp::webview_of(view));
                            }
                        } else if let Some(view) = target(main_view(&shell), &children, &overlays, &to) {
                            let wv = cdp::webview_of(view);
                            let tx = ev_tx.clone();
                            let from = to.clone();
                            let dims = cast_dims.clone();
                            if let Some(cast) = cdp::start(&wv, move |data, w, h| {
                                dims.set((w, h));
                                let _ = tx.send(Ev::Frame {
                                    from: from.clone(),
                                    data,
                                    w: w as u32,
                                    h: h as u32,
                                });
                            }) {
                                casts.insert(to.clone(), cast);
                            } else {
                                shikisha_core::append_hook_log(&shikisha_core::i18n::t(
                                    "err.browser.log_screencast_failed",
                                ));
                            }
                        }
                    } else if let Some(cast) = casts.remove(&to) {
                        // Give the page its own shape back before the stream goes away
                        if naturals.remove(&to).is_some() {
                            if let Some(view) = target(main_view(&shell), &children, &overlays, &to) {
                                cdp::call(
                                    &cdp::webview_of(view),
                                    "Emulation.clearDeviceMetricsOverride",
                                    "{}",
                                );
                            }
                        }
                        cdp::stop(cast);
                    }
                }
                Cmd::Inject { to, input } => {
                    if let Some(view) = target(main_view(&shell), &children, &overlays, &to) {
                        let wv = cdp::webview_of(view);
                        let (cw, ch) = cast_dims.get();
                        match input {
                            Input::Mouse { phase, x, y, down } => {
                                let (px, py) = (x * cw, y * ch);
                                let (kind, buttons) = match phase.as_str() {
                                    "pressed" => {
                                        mouse_down = true;
                                        ("mousePressed", 1)
                                    }
                                    "released" => {
                                        mouse_down = false;
                                        ("mouseReleased", 0)
                                    }
                                    _ => ("mouseMoved", if down || mouse_down { 1 } else { 0 }),
                                };
                                let params = serde_json::json!({
                                    "type": kind, "x": px, "y": py,
                                    "button": "left", "buttons": buttons, "clickCount": 1,
                                })
                                .to_string();
                                cdp::call(&wv, "Input.dispatchMouseEvent", &params);
                            }
                            Input::Wheel { x, y, dx, dy } => {
                                let params = serde_json::json!({
                                    "type": "mouseWheel", "x": x * cw, "y": y * ch,
                                    "deltaX": dx, "deltaY": dy,
                                })
                                .to_string();
                                cdp::call(&wv, "Input.dispatchMouseEvent", &params);
                            }
                            Input::Text { text } => {
                                // insertText doesn't land in the input fields
                                // of some sites, e.g. Google (they ignore the
                                // input event). Sending one char key event
                                // per character gets treated as a real
                                // keystroke and works much more broadly.
                                // IME conversion is already done on the sender's side, so just send the committed characters through
                                for ch in text.chars() {
                                    let mut buf = [0u8; 4];
                                    let s: &str = ch.encode_utf8(&mut buf);
                                    let params =
                                        serde_json::json!({ "type": "char", "text": s }).to_string();
                                    cdp::call(&wv, "Input.dispatchKeyEvent", &params);
                                }
                            }
                            Input::View { w, h } => {
                                // A phone reported its screen shape. Re-shape this page's
                                // viewport to that aspect while keeping the PC-side width,
                                // so the relay fills the phone's screen instead of leaving
                                // the bottom black. Cleared when the cast ends. The first
                                // report must come after a frame (cast_dims filled), which
                                // the sender guarantees
                                let (cw, ch) = cast_dims.get();
                                if cw >= 1.0 && ch >= 1.0 {
                                    let nat = *naturals.entry(to.clone()).or_insert((cw, ch));
                                    let want_h = (nat.0 * (h / w).clamp(0.2, 3.0)).round();
                                    if want_h > nat.1 * 1.02 {
                                        cdp::call(
                                            &wv,
                                            "Emulation.setDeviceMetricsOverride",
                                            &format!(
                                                "{{\"width\":{},\"height\":{},\"deviceScaleFactor\":0,\"mobile\":false}}",
                                                nat.0.round(),
                                                want_h
                                            ),
                                        );
                                    } else {
                                        // e.g. rotated to landscape — the real shape is fine
                                        cdp::call(&wv, "Emulation.clearDeviceMetricsOverride", "{}");
                                    }
                                }
                            }
                            Input::Key { named, ctrl, alt } => {
                                if let Some((key, vk)) = named_vk(&named) {
                                    // CDP modifier bits: Alt=1, Ctrl=2, Meta=4, Shift=8
                                    let mods = (if alt { 1 } else { 0 }) | (if ctrl { 2 } else { 0 });
                                    for kind in ["keyDown", "keyUp"] {
                                        let mut ev = serde_json::json!({
                                            "type": kind, "key": key,
                                            "windowsVirtualKeyCode": vk,
                                            "nativeVirtualKeyCode": vk,
                                            "modifiers": mods,
                                        });
                                        // Space needs a `text` field attached, or it won't land in
                                        // the input field. When combined with a modifier (e.g. Ctrl+Space), treat it as a shortcut instead
                                        if kind == "keyDown" && named == "space" && mods == 0 {
                                            ev["text"] = serde_json::Value::from(" ");
                                        }
                                        cdp::call(&wv, "Input.dispatchKeyEvent", &ev.to_string());
                                    }
                                }
                            }
                        }
                    }
                }
                Cmd::Hide => {
                    window.set_visible(false);
                    // Let go of the board's page, and of everything holding a
                    // reference to it -- a reference kept would keep Chromium's
                    // processes alive, and with them the memory this is for
                    wakes.remove(&None);
                    casts.remove(&None);
                    auths.remove(&None);
                    dialogs.remove(&None);
                    // The page's container window outlives the page: wry
                    // destroys it for a child page and not for a top-level
                    // one (wry 0.56), so every put-away left one more empty
                    // WRY_WEBVIEW child under the board that came back
                    let stale = {
                        use wry::WebViewExtWindows;
                        main_view(&shell).map(|v| v.hwnd().0)
                    };
                    shell = None;
                    if let Some(h) = stale {
                        use windows_sys::Win32::UI::WindowsAndMessaging::DestroyWindow;
                        let _ = unsafe { DestroyWindow(h) };
                    }
                    shikisha_core::append_hook_log("Board page dropped while the window is put away");
                }
                Cmd::Show => {
                    if shell.is_none() {
                        let born = std::time::Instant::now();
                        match shell_of() {
                            Ok(s) => {
                                shell = Some(s);
                                // The page made last sits on top of the ones
                                // made before it, so the new board would cover
                                // every page placed inside the window. Put
                                // those back above it, each overlay above the
                                // page it stands on
                                {
                                    use windows_sys::Win32::UI::WindowsAndMessaging::{
                                        HWND_TOP, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
                                        SetWindowPos,
                                    };
                                    use wry::WebViewExtWindows;
                                    let raise = |v: &wry::WebView| unsafe {
                                        SetWindowPos(
                                            v.hwnd().0,
                                            HWND_TOP,
                                            0,
                                            0,
                                            0,
                                            0,
                                            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                                        );
                                    };
                                    for v in children.values() {
                                        raise(v);
                                    }
                                    for stack in overlays.values() {
                                        for (_, v) in stack {
                                            raise(v);
                                        }
                                    }
                                }
                                shikisha_core::append_hook_log(&format!(
                                    "Board page built again in {} ms",
                                    born.elapsed().as_millis()
                                ));
                            }
                            Err(e) => shikisha_core::append_hook_log(&format!(
                                "Board page could not be built again: {e:#}"
                            )),
                        }
                    }
                    window.set_visible(true);
                    if window.is_minimized() {
                        window.set_minimized(false);
                    }
                    window.set_focus();
                }
                Cmd::TrayNotice { title, text } => tray.notice(&title, &text),
                Cmd::Close => {
                    *control = ControlFlow::Exit;
                }
            },
            // The ✕. Not the end of the program: the conductor answers with
            // `Hide` or `Close`, having asked the person if an AI is at work
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                let _ = ev_tx.send(Ev::CloseRequested);
            }
            // The board follows the window's size from here, not from the
            // runtime's own hook. wry sizes a top-level page by subclassing
            // the window for WM_SIZE -- and takes that subclass down again
            // whenever ANY page in the window is dropped, a child page
            // included (InnerWebView::drop detaches the parent subclass
            // unconditionally, wry 0.56). So from the first closed settings
            // page or browser tab onward, maximizing grew the window frame
            // and left the board at the size it was: the "pane widens, the
            // body does not" picture (2026-09-09). Setting the bounds here on
            // every Resized is the same call the hook made, from a place
            // nothing takes down. Doing it twice while the hook still lives
            // costs nothing
            Event::WindowEvent {
                event: WindowEvent::Resized(size),
                ..
            } => {
                if size.width > 0 && size.height > 0 {
                    if let Some(v) = main_view(&shell) {
                        let _ = v.set_bounds(wry::Rect {
                            position: wry::dpi::PhysicalPosition::new(0, 0).into(),
                            size: wry::dpi::PhysicalSize::new(size.width, size.height).into(),
                        });
                    }
                }
            }
            // The move notice the same hook gave, for the same reason:
            // Chromium places its popups (select lists, tooltips) by where
            // it last heard the window was, and a moved window whose page
            // was never told drops them in the old place. The hook's third
            // duty, handing focus to the board when the window gets it, is
            // left alone on purpose: Windows already gives focus back to
            // the page that had it, and a settings page being typed into
            // must not lose it to the board on every Alt+Tab
            Event::WindowEvent {
                event: WindowEvent::Moved(_),
                ..
            } => {
                use wry::WebViewExtWindows;
                if let Some(v) = main_view(&shell) {
                    let _ = unsafe { v.controller().NotifyParentWindowPositionChanged() };
                }
            }
            _ => {}
        }
    });

    tray.remove();
    let _ = closed_tx.send(Ev::Closed);
    Ok(())
}

/// Screencasting and input injection over CDP (Chrome DevTools Protocol).
///
/// WebView2 is Chromium under the hood, so it speaks the developer-tools
/// protocol. Using it lets us receive "only what changed" as JPEG frames
/// (lighter than VNC), and inject mouse, wheel, and text input as
/// **genuine input** (not synthetic events).
///
/// COM objects are thread-bound, so calls must always be made from the
/// window's event-loop thread (inside `run_window`). Frame notifications also arrive on that same thread.
mod cdp {
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        ICoreWebView2, ICoreWebView2DevToolsProtocolEventReceivedEventArgs,
        ICoreWebView2DevToolsProtocolEventReceiver,
    };
    use webview2_com::{
        CallDevToolsProtocolMethodCompletedHandler, DevToolsProtocolEventReceivedEventHandler,
    };
    use windows::core::{HSTRING, PCWSTR};

    /// Screencast parameters. maxHeight leaves headroom for a portrait-shaped
    /// (phone-viewer) viewport, so tall frames aren't scaled down and blurred
    const CAST_PARAMS: &str =
        "{\"format\":\"jpeg\",\"quality\":60,\"maxWidth\":1600,\"maxHeight\":2400,\"everyNthFrame\":1}";

    /// Wake parameters: the cheapest cast that still forces the compositor
    /// to produce frames. The frames themselves are thrown away — the point
    /// is that a hidden page becomes able to process (and ack) mouse input
    pub const WAKE_PARAMS: &str =
        "{\"format\":\"jpeg\",\"quality\":10,\"maxWidth\":32,\"maxHeight\":32,\"everyNthFrame\":10}";

    /// What's needed to tear down a screencast (notifications only arrive while this is held)
    pub struct Cast {
        pub receiver: ICoreWebView2DevToolsProtocolEventReceiver,
        pub token: i64,
        pub webview: ICoreWebView2,
    }

    /// Call one CDP method (the result is discarded). `params_json` can just be "{}"
    pub fn call(webview: &ICoreWebView2, method: &str, params_json: &str) {
        let method = HSTRING::from(method);
        let params = HSTRING::from(params_json);
        let handler =
            CallDevToolsProtocolMethodCompletedHandler::create(Box::new(|_hr, _json| Ok(())));
        unsafe {
            let _ = webview.CallDevToolsProtocolMethod(
                PCWSTR(method.as_ptr()),
                PCWSTR(params.as_ptr()),
                &handler,
            );
        }
    }

    /// Call one CDP method and hand its result to `done(ok, json)`.
    ///
    /// `done` is guaranteed to run exactly once: either from the completion
    /// handler, or right here when the call can't even be issued (in which
    /// case the handler would never fire and a waiter would hang until its
    /// timeout for no reason)
    pub fn call_result<F>(webview: &ICoreWebView2, method: &str, params_json: &str, done: F)
    where
        F: FnOnce(bool, String) + 'static,
    {
        let method_h = HSTRING::from(method);
        let params = HSTRING::from(params_json);
        let done = std::rc::Rc::new(std::cell::RefCell::new(Some(done)));
        let in_handler = std::rc::Rc::clone(&done);
        let context = format!("{method}");
        let handler = CallDevToolsProtocolMethodCompletedHandler::create(Box::new(
            move |hr: windows::core::Result<()>, json: String| {
                if let Some(f) = in_handler.borrow_mut().take() {
                    match hr {
                        Ok(()) => f(true, json),
                        Err(e) => f(false, format!("{context}: {e:?} {json}")),
                    }
                }
                Ok(())
            },
        ));
        let issued = unsafe {
            webview.CallDevToolsProtocolMethod(
                PCWSTR(method_h.as_ptr()),
                PCWSTR(params.as_ptr()),
                &handler,
            )
        };
        if let Err(e) = issued {
            if let Some(f) = done.borrow_mut().take() {
                f(false, format!("{method}: {e:?}"));
            }
        }
    }

    /// Pull the underlying `ICoreWebView2` out of a wry `WebView`
    pub fn webview_of(view: &wry::WebView) -> ICoreWebView2 {
        use wry::WebViewExtWindows;
        view.webview()
    }

    /// Start screencasting. Calls `on_frame(base64_jpeg, css_w, css_h)`
    /// every time a frame arrives.
    /// This also sends the frame ack automatically (without it, the next frame never comes)
    pub fn start<F>(webview: &ICoreWebView2, on_frame: F) -> Option<Cast>
    where
        F: FnMut(String, f64, f64) + 'static,
    {
        start_with(webview, CAST_PARAMS, on_frame)
    }

    /// `start` with explicit cast parameters (the wake path wants tiny frames)
    pub fn start_with<F>(webview: &ICoreWebView2, params: &str, on_frame: F) -> Option<Cast>
    where
        F: FnMut(String, f64, f64) + 'static,
    {
        let cb = std::rc::Rc::new(std::cell::RefCell::new(on_frame));
        let wv = webview.clone();
        let handler = DevToolsProtocolEventReceivedEventHandler::create(Box::new(
            move |_sender, args: Option<ICoreWebView2DevToolsProtocolEventReceivedEventArgs>| {
                if let Some(args) = args {
                    let mut raw = windows::core::PWSTR::null();
                    unsafe {
                        if args.ParameterObjectAsJson(&mut raw).is_ok() {
                            let json = webview2_com::take_pwstr(raw);
                            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&json) {
                                let data = v
                                    .get("data")
                                    .and_then(|x| x.as_str())
                                    .unwrap_or_default()
                                    .to_string();
                                let meta = v.get("metadata");
                                let w = meta
                                    .and_then(|m| m.get("deviceWidth"))
                                    .and_then(|x| x.as_f64())
                                    .unwrap_or(0.0);
                                let h = meta
                                    .and_then(|m| m.get("deviceHeight"))
                                    .and_then(|x| x.as_f64())
                                    .unwrap_or(0.0);
                                let sid = v.get("sessionId").and_then(|x| x.as_i64()).unwrap_or(0);
                                // Send the ack first, then deliver the frame (avoids stalling the pipe)
                                call(
                                    &wv,
                                    "Page.screencastFrameAck",
                                    &format!("{{\"sessionId\":{sid}}}"),
                                );
                                if !data.is_empty() {
                                    (cb.borrow_mut())(data, w, h);
                                }
                            }
                        }
                    }
                }
                Ok(())
            },
        ));

        let name = HSTRING::from("Page.screencastFrame");
        let mut token = 0i64;
        unsafe {
            let receiver = webview
                .GetDevToolsProtocolEventReceiver(PCWSTR(name.as_ptr()))
                .ok()?;
            receiver
                .add_DevToolsProtocolEventReceived(&handler, &mut token)
                .ok()?;
            call(webview, "Page.enable", "{}");
            call(webview, "Page.startScreencast", params);
            Some(Cast {
                receiver,
                token,
                webview: webview.clone(),
            })
        }
    }

    /// Basic-auth arming (401s only get answered while this is held).
    ///
    /// Receiving auth challenges (authRequired) requires intercepting
    /// requests, so we catch every request via Fetch. A caught, ordinary
    /// request is passed straight through with continueRequest (holding
    /// it forever would stall the page); only auth challenges get
    /// credentials back via continueWithAuth
    pub struct AuthArm {
        pub receivers: Vec<(ICoreWebView2DevToolsProtocolEventReceiver, i64)>,
        pub webview: ICoreWebView2,
        /// The credentials to return (user, pass). Held shared so it can be swapped out
        pub creds: std::rc::Rc<std::cell::RefCell<(String, String)>>,
    }

    /// Subscribe to one CDP event. Calls `on` with the received JSON
    fn subscribe<F>(
        webview: &ICoreWebView2,
        event: &str,
        on: F,
    ) -> Option<(ICoreWebView2DevToolsProtocolEventReceiver, i64)>
    where
        F: Fn(&serde_json::Value) + 'static,
    {
        let handler = DevToolsProtocolEventReceivedEventHandler::create(Box::new(
            move |_sender, args: Option<ICoreWebView2DevToolsProtocolEventReceivedEventArgs>| {
                if let Some(args) = args {
                    let mut raw = windows::core::PWSTR::null();
                    unsafe {
                        if args.ParameterObjectAsJson(&mut raw).is_ok() {
                            let json = webview2_com::take_pwstr(raw);
                            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&json) {
                                on(&v);
                            }
                        }
                    }
                }
                Ok(())
            },
        ));
        let name = HSTRING::from(event);
        let mut token = 0i64;
        unsafe {
            let receiver = webview
                .GetDevToolsProtocolEventReceiver(PCWSTR(name.as_ptr()))
                .ok()?;
            receiver
                .add_DevToolsProtocolEventReceived(&handler, &mut token)
                .ok()?;
            Some((receiver, token))
        }
    }

    pub fn arm_basic_auth(webview: &ICoreWebView2, user: &str, pass: &str) -> Option<AuthArm> {
        let creds = std::rc::Rc::new(std::cell::RefCell::new((user.to_string(), pass.to_string())));

        // Pass a caught, ordinary request straight through (not continuing it would stall the page)
        let wv_req = webview.clone();
        let paused = subscribe(webview, "Fetch.requestPaused", move |v| {
            if let Some(id) = v.get("requestId").and_then(|x| x.as_str()) {
                call(&wv_req, "Fetch.continueRequest", &format!("{{\"requestId\":\"{id}\"}}"));
            }
        })?;

        // Return credentials for auth challenges
        let wv_auth = webview.clone();
        let creds_h = std::rc::Rc::clone(&creds);
        let required = subscribe(webview, "Fetch.authRequired", move |v| {
            let id = v.get("requestId").and_then(|x| x.as_str()).unwrap_or_default();
            let (u, p) = {
                let c = creds_h.borrow();
                (c.0.clone(), c.1.clone())
            };
            let params = serde_json::json!({
                "requestId": id,
                "authChallengeResponse": {
                    "response": "ProvideCredentials",
                    "username": u,
                    "password": p,
                }
            })
            .to_string();
            call(&wv_auth, "Fetch.continueWithAuth", &params);
        })?;

        // Catch every request, and auth too
        call(
            webview,
            "Fetch.enable",
            r#"{"patterns":[{"urlPattern":"*"}],"handleAuthRequests":true}"#,
        );
        Some(AuthArm {
            receivers: vec![paused, required],
            webview: webview.clone(),
            creds,
        })
    }

    /// Automatic handling of JS dialogs (alert / confirm / prompt / beforeunload).
    ///
    /// Without this, things like a page's "leave this page?" confirmation
    /// open as a native dialog, the CDP response channel stalls, and
    /// `browser_*` hangs with "no result returned" (automation freezes
    /// entirely). Since this is for automation, the default is
    /// accept=true = proceed: beforeunload means "navigate away", confirm
    /// means OK, alert/prompt means dismiss. Once `Page` is enabled and
    /// this is subscribed, no more native dialogs appear — we close them
    /// immediately instead. Only active while this is held (unsubscribes on drop).
    pub struct DialogArm {
        pub receivers: Vec<(ICoreWebView2DevToolsProtocolEventReceiver, i64)>,
        pub webview: ICoreWebView2,
    }

    pub fn arm_dialogs(webview: &ICoreWebView2) -> Option<DialogArm> {
        let wv = webview.clone();
        let opening = subscribe(webview, "Page.javascriptDialogOpening", move |_v| {
            call(&wv, "Page.handleJavaScriptDialog", r#"{"accept":true}"#);
        })?;
        // Enable Page so the subscription actually fires (idempotent even if screencast already enabled it)
        call(webview, "Page.enable", "{}");
        Some(DialogArm {
            receivers: vec![opening],
            webview: webview.clone(),
        })
    }

    /// Force the current screen out as one frame (re-issues startScreencast).
    /// Used when a new viewer joins but the page is static and no new change is coming
    pub fn kick(webview: &ICoreWebView2) {
        call(webview, "Page.startScreencast", CAST_PARAMS);
    }

    /// Stop the screencast and unsubscribe its notifications too
    pub fn stop(cast: Cast) {
        unsafe {
            call(&cast.webview, "Page.stopScreencast", "{}");
            let _ = cast
                .receiver
                .remove_DevToolsProtocolEventReceived(cast.token);
        }
    }
}

#[cfg(test)]
mod nav_tests {
    use super::*;

    /// Fill in a missing scheme, and never allow anything but http/https.
    ///
    /// The URL bar is a "gateway to anywhere", so opening `file:` would
    /// expose local files and `javascript:` could hijack the current
    /// page — and from there, automation would be exposed to it.
    /// The destination is narrowed down right here
    #[test]
    fn the_address_box_only_opens_web_pages() {
        assert_eq!(openable("example.com").as_deref(), Some("https://example.com"));
        assert_eq!(
            openable("  https://a.example/x?y=1  ").as_deref(),
            Some("https://a.example/x?y=1"),
            "前後の空白は落とす"
        );
        assert_eq!(
            openable("http://127.0.0.1:8080/").as_deref(),
            Some("http://127.0.0.1:8080/")
        );
        assert_eq!(
            openable("HTTPS://Example.com/A").as_deref(),
            Some("https://Example.com/A"),
            "貼り付けた大文字スキームも通す（後段の検査は小文字前提）"
        );
        for empty in ["", "   "] {
            assert!(openable(empty).is_none(), "開けてしまう: {empty}");
        }
        // Dangerous schemes never reach the page — they become an inert search instead
        for bad in ["file:///C:/secret.txt", "ftp://x/y", "javascript:alert(1)"] {
            let got = openable(bad).unwrap_or_default();
            assert!(
                got.starts_with("https://www.google.com/search?q="),
                "検索に落ちていない: {bad} -> {got}"
            );
        }
    }

    /// Text that doesn't read as an address searches Google instead — same
    /// habit as Chrome's box. Japanese (multibyte) must survive as UTF-8
    /// percent-encoding, and spaces split words with `+`
    #[test]
    fn the_address_box_searches_words() {
        assert_eq!(
            openable("エラー処理").as_deref(),
            Some("https://www.google.com/search?q=%E3%82%A8%E3%83%A9%E3%83%BC%E5%87%A6%E7%90%86")
        );
        assert_eq!(
            openable("rust async 使い方").as_deref(),
            Some("https://www.google.com/search?q=rust+async+%E4%BD%BF%E3%81%84%E6%96%B9")
        );
        // A dot inside a phrase with spaces is still a search, not an address
        assert_eq!(
            openable("tokio.rs とは").as_deref(),
            Some("https://www.google.com/search?q=tokio.rs+%E3%81%A8%E3%81%AF")
        );
        // A lone word with no dot searches; localhost is the address exception
        let one = openable("rust").unwrap_or_default();
        assert!(one.starts_with("https://www.google.com/search?q=rust"), "{one}");
        assert_eq!(
            openable("localhost:8080/x").as_deref(),
            Some("https://localhost:8080/x")
        );
        // Query characters that would break the search URL are encoded
        assert_eq!(
            openable("a&b=c").as_deref(),
            Some("https://www.google.com/search?q=a%26b%3Dc")
        );
    }

    /// The wheel's signal reads as an amount to scroll back through the log.
    /// Turning it up goes to the past (positive), turning it down goes to now (negative)
    #[test]
    fn the_wheel_asks_to_go_back_through_the_log() {
        let read = |s: &str| {
            let v: serde_json::Value = serde_json::from_str(s).unwrap();
            parse_intent(&v)
        };
        assert!(matches!(
            read(r#"{"kind":"scroll","by":3,"row":4,"col":9}"#),
            Some(Ev::Scroll { by: 3, row: 4, col: 9 })
        ));
        assert!(matches!(
            read(r#"{"kind":"scroll","by":-3,"row":0,"col":0}"#),
            Some(Ev::Scroll { by: -3, .. })
        ));
        // With no amount it doesn't move (0 means "do nothing", not "discard")
        assert!(matches!(read(r#"{"kind":"scroll"}"#), Some(Ev::Scroll { by: 0, .. })));
        // Clamp amounts beyond a tall phone's page turn (≈ one tick per row)
        assert!(matches!(
            read(r#"{"kind":"scroll","by":999999}"#),
            Some(Ev::Scroll { by: 250, .. })
        ));
    }

    /// A signal from the screen becomes a navigation instruction as-is
    #[test]
    fn the_bar_speaks_the_same_words_as_the_loop() {
        let read = |s: &str| {
            let v: serde_json::Value = serde_json::from_str(s).unwrap();
            parse_intent(&v)
        };
        assert!(matches!(
            read(r#"{"kind":"go","what":"back"}"#),
            Some(Ev::Go { go: Go::Back })
        ));
        assert!(matches!(
            read(r#"{"kind":"go","what":"reload"}"#),
            Some(Ev::Go { go: Go::Reload })
        ));
        // The same button, held down. A page that is wrong from a build that
        // has moved on is exactly when someone reaches for it
        assert!(matches!(
            read(r#"{"kind":"go","what":"hardreload"}"#),
            Some(Ev::Go { go: Go::Hard })
        ));
        match read(r#"{"kind":"go","what":"to","url":"example.com"}"#) {
            Some(Ev::Go { go: Go::To(u) }) => assert_eq!(u, "example.com"),
            other => panic!("行き先が読めていない: {other:?}"),
        }
        // Discard unknown instructions. Doing nothing is better than silently doing something else
        assert!(read(r#"{"kind":"go","what":"quit"}"#).is_none());
        assert!(read(r#"{"kind":"go"}"#).is_none());
    }

    /// A chosen name has to be told the same way twice, or the second telling
    /// gives away the first.
    #[test]
    fn what_the_browser_says_it_is_agrees_with_itself() {
        let chrome = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                      (KHTML, like Gecko) Chrome/151.0.0.0 Safari/537.36";
        let v: serde_json::Value = serde_json::from_str(&ua_override(chrome)).unwrap();
        let brands = v["userAgentMetadata"]["brands"].as_array().unwrap();
        let names: Vec<&str> = brands.iter().filter_map(|b| b["brand"].as_str()).collect();
        assert!(names.contains(&"Google Chrome"), "{names:?}");
        assert!(!names.iter().any(|n| n.contains("WebView")), "名乗りと食い違う: {names:?}");
        assert_eq!(brands[0]["version"], "151");

        // Edge names itself twice; both have to be there or the pair is odd
        let edge = format!("{chrome} Edg/151.0.0.0");
        let v: serde_json::Value = serde_json::from_str(&ua_override(&edge)).unwrap();
        let names: Vec<&str> = v["userAgentMetadata"]["brands"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|b| b["brand"].as_str())
            .collect();
        assert!(names.contains(&"Microsoft Edge") && names.contains(&"Chromium"), "{names:?}");

        // Something that is not a Chromium at all sends no brands, which is
        // exactly what such a browser does
        let v: serde_json::Value =
            serde_json::from_str(&ua_override("Mozilla/5.0 … Firefox/999.0")).unwrap();
        assert!(v["userAgentMetadata"]["brands"].as_array().unwrap().is_empty());
    }

    /// A phone reporting its screen shape parses into a View input, and a
    /// nonsense size can never divide by zero downstream (floors at 1)
    #[test]
    fn a_viewer_screen_shape_parses() {
        let v: serde_json::Value =
            serde_json::from_str(r#"{"kind":"inject","what":"view","w":390,"h":780}"#).unwrap();
        match parse_intent(&v) {
            Some(Ev::Inject { input: Input::View { w, h }, .. }) => {
                assert_eq!((w, h), (390.0, 780.0));
            }
            other => panic!("画面の形が読めていない: {other:?}"),
        }
        let z: serde_json::Value =
            serde_json::from_str(r#"{"kind":"inject","what":"view","w":0,"h":-5}"#).unwrap();
        match parse_intent(&z) {
            Some(Ev::Inject { input: Input::View { w, h }, .. }) => {
                assert!(w >= 1.0 && h >= 1.0, "ゼロ割りの芽: {w}x{h}");
            }
            other => panic!("画面の形が読めていない: {other:?}"),
        }
    }

    /// The workspace button and the model-chat box parse into their own intents,
    /// not into a keystroke that would leak into the visible session.
    #[test]
    fn workspace_and_chat_intents_parse() {
        let read = |s: &str| {
            let v: serde_json::Value = serde_json::from_str(s).unwrap();
            parse_intent(&v)
        };
        assert!(matches!(read(r#"{"kind":"openws"}"#), Some(Ev::OpenWs)));
        // The gear names the tab in view by its place in the folder
        match read(r#"{"kind":"opensettings","tabpos":1,"folder":"D:/work"}"#) {
            Some(Ev::OpenSettings { tabpos, folder, section, ret }) => {
                assert_eq!(tabpos, Some(1), "見ていたタブの位置が落ちた");
                assert_eq!(folder.as_deref(), Some("D:/work"));
                assert!(section.is_none() && !ret);
            }
            other => panic!("opensettings が読めていない: {other:?}"),
        }
        match read(r#"{"kind":"opensettings"}"#) {
            Some(Ev::OpenSettings { tabpos, .. }) => assert!(tabpos.is_none(), "位置が無いのに入った"),
            other => panic!("opensettings が読めていない: {other:?}"),
        }
        match read(r#"{"kind":"say","tab":3,"text":"hello"}"#) {
            Some(Ev::Say { tab, text }) => {
                assert_eq!((tab, text.as_str()), (3, "hello"), "宛名と本文が揃っていない");
            }
            other => panic!("say が読めていない: {other:?}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// A page placed in the window may report, and may not ask. Every intent
    /// that types, runs, changes or opens something is refused from a
    /// stranger's page; the handful of reports our own scripts post from inside
    /// a page still come through. This is the list itself, pinned: a variant
    /// added to `Ev` is refused from a page until somebody writes down why not
    #[test]
    fn a_stranger_page_may_report_but_never_ask() {
        let read = |s: &str| {
            let v: serde_json::Value = serde_json::from_str(s).unwrap();
            parse_intent(&v).unwrap_or_else(|| panic!("parse_intent が読めない: {s}"))
        };
        for s in [
            r#"{"kind":"say","tab":1,"text":"rm -rf ~"}"#,
            r#"{"kind":"key","text":"x"}"#,
            r#"{"kind":"runlua","code":"print(1)"}"#,
            r#"{"kind":"runaction","index":0}"#,
            r#"{"kind":"runkey","name":"x"}"#,
            r#"{"kind":"git","panel":"a","act":"commit"}"#,
            r#"{"kind":"opensettings"}"#,
            r#"{"kind":"closesettings"}"#,
            r#"{"kind":"select","tab":2}"#,
            r#"{"kind":"menu","key":"q"}"#,
            r#"{"kind":"operate","target":1,"goal":"x"}"#,
            r#"{"kind":"attach","id":1,"name":"a","data":"b"}"#,
            r#"{"kind":"paste"}"#,
            r#"{"kind":"copy","text":"x"}"#,
            r#"{"kind":"stop"}"#,
            r#"{"kind":"restart"}"#,
            r#"{"kind":"go","what":"to","url":"https://example.com"}"#,
            r#"{"kind":"inject","what":"text","text":"x"}"#,
            r#"{"kind":"suggest","text":"x"}"#,
            r#"{"kind":"password","text":"x"}"#,
            r#"{"kind":"addtab"}"#,
            r#"{"kind":"branch","from":"a","branch":"b"}"#,
            // The bar's press: a page saying "the person pressed it" is the
            // one report that must never be believed
            r#"{"kind":"button","name":"web"}"#,
        ] {
            assert!(!allowed_from_page(&read(s)), "よそのページから通ってしまう: {s}");
        }
        for s in [
            r#"{"kind":"touched"}"#,
            r#"{"kind":"compose"}"#,
            r##"{"kind":"recorded","act":"click","sel":"#a"}"##,
            r#"{"kind":"ready","url":"https://example.com"}"#,
            r#"{"kind":"loading","busy":false}"#,
            r#"{"kind":"result","id":1,"ok":true,"value":"x"}"#,
        ] {
            assert!(allowed_from_page(&read(s)), "ページの報告が落とされる: {s}");
        }
    }

    /// What a stranger's page says is sifted: its asks vanish, its reports
    /// arrive stamped with the pane's name (whatever the message claimed), and
    /// the app's own page — judged by the address it spoke from — keeps its
    /// full voice. Broken JSON and unknown kinds are dropped without a word
    #[test]
    fn a_placed_page_is_heard_only_as_a_report() {
        let mut asked = Asked::default();
        let stranger = |body: &str, asked: &mut Asked| heard(body, Some("web"), false, asked);
        // The very message the review found going into the terminal
        assert!(
            stranger(r#"{"kind":"say","tab":1,"text":"SECURITY_REVIEW_MARKER"}"#, &mut asked)
                .is_none(),
            "よそのページの say が端末へ届く"
        );
        assert!(stranger(r#"{"kind":"key","text":"x"}"#, &mut asked).is_none());
        assert!(stranger(r#"{"kind":"runlua","code":"1"}"#, &mut asked).is_none());
        assert!(stranger("not json", &mut asked).is_none());
        assert!(stranger(r#"{"kind":"nosuchthing"}"#, &mut asked).is_none());
        // A page cannot press the bar for the person
        assert!(
            stranger(r#"{"kind":"button","name":"web"}"#, &mut asked).is_none(),
            "ページが帯のボタンを押せる"
        );
        // A report gets the pane's name, not the one it wrote
        match stranger(r#"{"kind":"touched","from":"settings"}"#, &mut asked) {
            Some(Ev::Touched { from }) => assert_eq!(from.as_deref(), Some("web")),
            other => panic!("フォーカスの報告が届かない: {other:?}"),
        }
        match stranger(r#"{"kind":"ready","url":"https://a.example/"}"#, &mut asked) {
            Some(Ev::Ready { from, url, .. }) => {
                assert_eq!(from.as_deref(), Some("web"));
                assert_eq!(url, "https://a.example/");
            }
            other => panic!("読み込み完了の報告が届かない: {other:?}"),
        }
        // Our own page, speaking from our address, still asks
        assert!(matches!(
            heard(r#"{"kind":"closesettings"}"#, Some("settings"), true, &mut asked),
            Some(Ev::CloseSettings)
        ));
        assert!(matches!(
            heard(r#"{"kind":"select","tab":0}"#, Some("settings"), true, &mut asked),
            Some(Ev::Select { tab: 0 })
        ));
        // The board's own reports carry no name -- except the bar's press,
        // which names the page the bar stands under
        assert!(matches!(
            heard(r#"{"kind":"say","tab":1,"text":"ls"}"#, None, true, &mut asked),
            Some(Ev::Say { tab: 1, .. })
        ));
        match heard(r#"{"kind":"button","name":"br"}"#, None, true, &mut asked) {
            Some(Ev::Button { from }) => assert_eq!(from.as_deref(), Some("br")),
            other => panic!("盤面の帯の押下が届かない: {other:?}"),
        }
    }

    /// An answer is believed only from the page that was asked, and only once.
    /// A neighbour's page guessing the id cannot answer for it; a second
    /// answer, even from the right page, is refused; and the bookkeeping does
    /// not grow without end
    #[test]
    fn an_answer_is_believed_only_from_the_page_that_was_asked() {
        let mut asked = Asked::default();
        asked.ask(7, Some("bank".into()));
        asked.ask(8, None);
        let answer = |id: u64| format!(r#"{{"kind":"result","id":{id},"ok":true,"value":"x"}}"#);
        // The neighbour answers first, with the right id
        assert!(
            heard(&answer(7), Some("evil"), false, &mut asked).is_none(),
            "隣のページが答えを差し替えられる"
        );
        // Then the page that was asked
        assert!(matches!(
            heard(&answer(7), Some("bank"), false, &mut asked),
            Some(Ev::Result { id: 7, ok: true, .. })
        ));
        // ...and nobody answers the same question twice
        assert!(heard(&answer(7), Some("bank"), false, &mut asked).is_none());
        // The board's question is not a page's to answer, nor a page's the board's
        assert!(heard(&answer(8), Some("bank"), true, &mut asked).is_none());
        assert!(matches!(
            heard(&answer(8), None, true, &mut asked),
            Some(Ev::Result { id: 8, .. })
        ));
        // A question nobody asked has no answer
        assert!(heard(&answer(9), None, true, &mut asked).is_none());
        // Bounded: the oldest question is forgotten past the cap
        let mut many = Asked::default();
        for id in 0..(Asked::KEPT as u64 + 10) {
            many.ask(id, None);
        }
        assert_eq!(many.of.len(), Asked::KEPT);
        assert!(!many.answered(0, None), "上限を超えても古い問いが残る");
        assert!(many.answered(Asked::KEPT as u64 + 9, None));
    }

    /// The app's pages come from two local servers: the board's, and the one
    /// serving the settings and the result view on a port of its own. A page
    /// from either is ours; a page from any other port on the same machine is
    /// not (a dev server in a browser tab is somebody else's code)
    #[test]
    fn our_pages_come_from_two_servers() {
        let own = vec!["http://127.0.0.1:8787/".to_string(), "http://127.0.0.1:51604/?token=x".to_string()];
        assert!(from_ours(&own, "http://127.0.0.1:8787/?token=abc"));
        assert!(from_ours(&own, "http://127.0.0.1:51604/?token=abc&ws=2"), "設定ページがよそ者扱い");
        assert!(from_ours(&own, "http://127.0.0.1:51604/result?token=abc&run=1"));
        assert!(!from_ours(&own, "http://127.0.0.1:3000/"), "同じ機械の別サーバが自前扱い");
        assert!(!from_ours(&own, "https://example.com/"));
        assert!(!from_ours(&[], "http://127.0.0.1:8787/"));
    }

    /// "The same site" is scheme, host and port, read by parsing — a prefix
    /// match would let `http://127.0.0.1:8787.evil.example/` pass as ours
    #[test]
    fn same_site_means_scheme_host_and_port() {
        let own = "http://127.0.0.1:8787/";
        assert!(same_origin("http://127.0.0.1:8787/?token=abc", own));
        assert!(same_origin("http://127.0.0.1:8787/settings?x=1#y", own));
        assert!(same_origin("HTTP://127.0.0.1:8787/", own));
        assert!(!same_origin("http://127.0.0.1:8788/", own), "別のポートが同一視される");
        assert!(!same_origin("https://127.0.0.1:8787/", own), "別のスキームが同一視される");
        assert!(!same_origin("http://localhost:8787/", own), "別のホスト名が同一視される");
        assert!(!same_origin("http://127.0.0.1:8787.evil.example/", own), "前方一致で通る");
        assert!(!same_origin("http://evil.example/http://127.0.0.1:8787/", own));
        assert!(!same_origin("about:blank", own));
        assert!(!same_origin("", own));
        assert!(!same_origin("garbage", own));
        // The usual port is the written port
        assert!(same_origin("https://a.example/", "https://a.example:443/x"));
        assert!(same_origin("http://a.example/", "http://a.example:80/x"));
        assert!(!same_origin("http://a.example/", "https://a.example/"));
    }

    /// A placed page can draw the app's own two pieces of chrome, because
    /// nothing of the window's can be drawn over it: the pen that summons the
    /// composer, and a message raised while that page is in front. Both live in
    /// the script every placed page is created with, and both hang off a name
    /// the app calls — rename one here and the window would go on calling into
    /// a page that no longer answers, silently.
    #[test]
    fn a_placed_page_can_draw_the_pen_and_a_message() {
        for name in ["__shikisha_pen", "__shikisha_toast"] {
            assert!(
                PLACED_JS.contains(&format!("window.{name} = function")),
                "{name} を置いていない"
            );
        }
        // In a shadow root, or the page's own CSS reaches it (and ours reaches
        // the page). The host is findable by id so it can be checked for.
        assert!(
            PLACED_JS.contains("toastEl.id = \"__shikisha_toast\"")
                && PLACED_JS.matches("attachShadow").count() >= 2,
            "影の中に置いていない / 名前が付いていない"
        );
    }


    /// Serve a test page on 127.0.0.1.
    /// `file:///` crashes on wry's IPC, so use http, same as production
    fn serve(body: &'static str) -> String {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        std::thread::spawn(move || {
            for req in server.incoming_requests() {
                let _ = req.respond(
                    tiny_http::Response::from_string(body).with_header(
                        tiny_http::Header::from_bytes(
                            &b"Content-Type"[..],
                            &b"text/html; charset=utf-8"[..],
                        )
                        .unwrap(),
                    ),
                );
            }
        });
        format!("http://127.0.0.1:{port}/")
    }

    const PAGE: &str = r#"<!doctype html><meta charset=utf-8><body>
<div id=here>ここにいる</div>
<input id=q value="">
<textarea id=multi></textarea>
<button id=go onclick="document.getElementById('log').textContent='pushed'">押す</button>
<div id=log></div>
<table><tr><td>氏名</td><td id=name>山田</td></tr></table>
<div style="height:4000px"></div>
<div id=far>ずっと下</div>
<script>
  var fired = 0;
  document.getElementById('q').addEventListener('input', function(){ fired++; });
</script>"#;

    /// Find it, click it, fill it, read it.
    ///
    ///   cargo test browser_page_ops -- --ignored --nocapture
    #[test]
    #[ignore]
    fn browser_page_ops() {
        let b = Browser::spawn(&serve(PAGE), "SHIKISHA-TERM ops probe").expect("窓が開かない");
        let t = 20_000;

        // Distinguish "not in the DOM" from "in the DOM but off-screen".
        // Collapsing them into one failure makes it impossible to tell whether to suspect the selector or the wait
        assert_eq!(b.find(None, &Sel::Css("#here".into()), t).unwrap(), Found::Visible);
        assert_eq!(b.find(None, &Sel::Css("#far".into()), t).unwrap(), Found::OffScreen);
        assert_eq!(b.find(None, &Sel::Css("#nope".into()), t).unwrap(), Found::NotFound);

        // XPath: a lookup CSS can't express (the cell next to a label)
        let name = b
            .text(None, &Sel::Xpath("//td[text()='氏名']/following-sibling::td".into()), t)
            .unwrap();
        assert_eq!(name.as_deref(), Some("山田"), "XPathで隣のセルが取れない");

        // Click it
        assert_eq!(b.click(None, &Sel::Css("#go".into()), t).unwrap().state, Found::Visible);
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert_eq!(
            b.text(None, &Sel::Css("#log".into()), t).unwrap().as_deref(),
            Some("pushed"),
            "押した結果がページに出ていない"
        );

        // Fill it. Not just writing the value — the `input` event must fire too
        // (frameworks like React won't update state otherwise)
        assert_eq!(
            b.fill(None, &Sel::Css("#q".into()), "ふつうの値", t).unwrap().state,
            Found::Visible
        );
        assert_eq!(
            b.text(None, &Sel::Css("#q".into()), t).unwrap().as_deref(),
            Some("ふつうの値")
        );
        let id = b.eval("return fired;").unwrap();
        assert_eq!(
            b.wait_result(id, std::time::Duration::from_millis(t)).unwrap(),
            "1",
            "input イベントが飛んでいない"
        );

        // This is the crux: the value must never become code.
        // Even AI output or text read straight off a page arrives as a plain value
        let nasty = "'; window.__pwned = 1; //\"</script><img src=x onerror=alert(1)>\\";
        assert_eq!(
            b.fill(None, &Sel::Css("#q".into()), nasty, t).unwrap().state,
            Found::Visible
        );
        assert_eq!(
            b.text(None, &Sel::Css("#q".into()), t).unwrap().as_deref(),
            Some(nasty),
            "値が一字一句そのまま入っていない"
        );

        // A value containing newlines. A single-line `input` drops
        // newlines (per the HTML spec), so multi-line values must go
        // through a `textarea`. The value isn't corrupted — the container just can't hold it
        let multi = format!("1行目\n2行目\t{nasty}");
        assert_eq!(
            b.fill(None, &Sel::Css("#multi".into()), &multi, t).unwrap().state,
            Found::Visible
        );
        assert_eq!(
            b.text(None, &Sel::Css("#multi".into()), t).unwrap().as_deref(),
            Some(multi.as_str()),
            "改行やタブを含む値が崩れている"
        );
        let id = b.eval("return typeof window.__pwned;").unwrap();
        assert_eq!(
            b.wait_result(id, std::time::Duration::from_millis(t)).unwrap(),
            "\"undefined\"",
            "渡した値がコードとして実行された"
        );

        // The full parsed HTML
        let html = b.html(None, t).unwrap();
        assert!(html.contains("ここにいる"), "HTMLが取れていない");
        assert!(html.len() > 200, "HTMLが短すぎる: {}", html.len());
        println!("HTML {} 文字 / すべて通過", html.chars().count());

        drop(b);
    }


    /// Pages can be placed inside the same window.
    ///
    ///   cargo test child_view -- --ignored --nocapture
    ///
    /// With a separate window, ownership, position tracking, and even
    /// exposure during Windows Terminal tab switching all became our own problem
    #[test]
    #[ignore]
    fn a_page_can_sit_inside_the_window() {
        let b = Browser::spawn(&serve(PAGE), "child probe").expect("窓が開かない");
        b.open_child("side", "https://example.com/", (400, 0, 400, 500), BrowserProfile::shared_default())
            .expect("置けない");
        std::thread::sleep(std::time::Duration::from_secs(3));
        // Its position can be changed
        b.child_bounds("side", (200, 0, 600, 500)).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(600));
        // Hidden with width 0
        b.child_bounds("side", (0, 0, 0, 0)).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(400));
        b.close_child("side").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(400));
        // The shell itself stays alive
        let id = b.eval("return 1+1;").unwrap();
        assert_eq!(
            b.wait_result(id, std::time::Duration::from_secs(10)).unwrap(),
            "2",
            "子を置いたら外皮が動かなくなった"
        );
        println!("子ページの出し入れ: 通過");
        drop(b);
    }

    /// URLs we can't open are stopped at the door.
    ///
    /// wry turns a page's URL into an `http::Uri` and unwraps it on IPC,
    /// so opening `file:///` or `data:` takes down the whole process the
    /// moment the initialization script sends its first message. Confirmed by testing
    #[test]
    fn only_http_pages_are_opened() {
        assert!(is_openable("https://example.com/a"));
        assert!(is_openable("http://127.0.0.1:8080/"));

        assert!(!is_openable("file:///C:/tmp/a.html"), "file: は落ちる");
        assert!(!is_openable("data:text/html,<b>x"), "data: は落ちる");
        assert!(!is_openable("about:blank"));
        assert!(!is_openable("https://"), "ホストが無い");
        assert!(!is_openable(""));
        assert!(!is_openable("https://example.com/a\nhttps://evil"), "改行の混入");
    }

    /// The window opens, JS runs, results come back, and closing it
    /// doesn't kill the app.
    ///
    ///   cargo test browser_round_trip -- --ignored --nocapture
    ///
    /// That last point is the crux. tao's `run` calls `process::exit`
    /// internally, so a naive implementation would take down the whole TUI just by closing the window
    #[test]
    #[ignore]
    fn browser_round_trip() {
        // Test through the same path as production. `file:///` crashes on wry's IPC, so it can't be used
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        std::thread::spawn(move || {
            for req in server.incoming_requests() {
                let body = "<title>t</title><body><div id=aaa>hello</div>";
                let _ = req.respond(
                    tiny_http::Response::from_string(body).with_header(
                        tiny_http::Header::from_bytes(
                            &b"Content-Type"[..],
                            &b"text/html; charset=utf-8"[..],
                        )
                        .unwrap(),
                    ),
                );
            }
        });
        let url = format!("http://127.0.0.1:{port}/");

        let b = Browser::spawn(&url, "SHIKISHA-TERM browser probe").expect("窓が開かない");

        let id = b.eval("return 40 + 2;").unwrap();
        let v = b.wait_result(id, Duration::from_secs(20)).expect("結果なし");
        println!("eval(40+2) = {v}");
        assert_eq!(v, "42");

        let id = b.eval("return document.querySelector('#aaa').textContent;").unwrap();
        let v = b.wait_result(id, Duration::from_secs(20)).expect("結果なし");
        println!("querySelector = {v}");
        assert_eq!(v, "\"hello\"");

        let id = b.eval("return document.documentElement.outerHTML.length;").unwrap();
        println!("HTML長 = {}", b.wait_result(id, Duration::from_secs(20)).unwrap());

        drop(b);
        std::thread::sleep(Duration::from_millis(600));
        println!("閉じてもここまで来た (プロセスは生きている)");
    }

    /// The CDP lane end-to-end: digest lists the operable elements (AX lane
    /// and JS-clickable lane both), `{ref=N}` clicks with a genuine mouse
    /// event that fires the page's own onclick, and ref-fill types multibyte
    /// text as real key events.
    ///
    ///   cargo test digest_round_trip -- --ignored --nocapture
    #[test]
    #[ignore]
    fn digest_round_trip() {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        std::thread::spawn(move || {
            for req in server.incoming_requests() {
                let body = r#"<title>t</title><body>
                  <button id="b" onclick="document.getElementById('log').textContent='clicked'">押す</button>
                  <a href="https://example.com/x">リンク</a>
                  <input id="i" placeholder="名前">
                  <div id="d" style="cursor:pointer" onclick="void 0">丸いやつ</div>
                  <a href="https://example.com/dup" onclick="return false">重複</a>
                  <a href="https://example.com/dup" onclick="return false">重複</a>
                  <div id="log"></div>"#;
                let _ = req.respond(
                    tiny_http::Response::from_string(body).with_header(
                        tiny_http::Header::from_bytes(
                            &b"Content-Type"[..],
                            &b"text/html; charset=utf-8"[..],
                        )
                        .unwrap(),
                    ),
                );
            }
        });
        let url = format!("http://127.0.0.1:{port}/");
        let b = Browser::spawn(&url, "SHIKISHA-TERM digest probe").expect("窓が開かない");

        let text = b.digest(None, 20_000).expect("digestが取れない");
        println!("{text}");
        assert!(text.contains("button \"押す\""), "ボタンがAXレーンから載る:\n{text}");
        assert!(text.contains("リンク") && text.contains("https://example.com/x"), "{text}");
        assert!(text.contains("名前"), "入力欄の名前(placeholder由来)が載る:\n{text}");
        assert!(text.contains("div*") && text.contains("丸いやつ"), "JSクリッカブルが補完される:\n{text}");

        // A line reads `[N] role "name" …` — pull N for the line matching `needle`
        let ref_of = |needle: &str| -> u32 {
            text.lines()
                .find(|l| l.contains(needle))
                .and_then(|l| l.strip_prefix('['))
                .and_then(|l| l.split(']').next())
                .and_then(|n| n.parse().ok())
                .unwrap_or_else(|| panic!("refが取れない: {needle}"))
        };

        // A genuine click fires the page's own onclick, and the echo names
        // what was clicked (a wrong ref number would answer for itself)
        let rb = ref_of("押す");
        let rep = b.click(None, &Sel::Ref(rb), 10_000).unwrap();
        assert_eq!(rep.state, Found::Visible);
        let echo = rep.echo.expect("refクリックはエコーを返す");
        assert!(
            echo.contains("button") && echo.contains("押す"),
            "何を押したか名乗る: {echo}"
        );
        // The durable anchor for the replay journal: the button has a
        // human-made id, so the anchor is its css form
        assert_eq!(
            rep.anchor,
            Some(("css".to_string(), "#b".to_string())),
            "idを持つ要素のアンカーは #id"
        );
        std::thread::sleep(Duration::from_millis(400));
        let id = b.eval("return document.getElementById('log').textContent;").unwrap();
        let v = b.wait_result(id, Duration::from_secs(10)).unwrap();
        assert_eq!(v, "\"clicked\"", "本物のマウスイベントがonclickを発火させる");

        // Ref-fill types multibyte as char key events; ref-text reads it back
        let ri = ref_of("名前");
        assert_eq!(b.fill(None, &Sel::Ref(ri), "俳句テスト", 10_000).unwrap().state, Found::Visible);
        std::thread::sleep(Duration::from_millis(400));
        let id = b.eval("return document.getElementById('i').value;").unwrap();
        let v = b.wait_result(id, Duration::from_secs(10)).unwrap();
        assert_eq!(v, "\"俳句テスト\"", "charキーイベントでマルチバイトが入る");
        assert_eq!(
            b.text(None, &Sel::Ref(ri), 10_000).unwrap().as_deref(),
            Some("俳句テスト")
        );

        // A stale/unknown ref refuses with guidance instead of clicking air
        let err = b.click(None, &Sel::Ref(999), 10_000).unwrap_err().to_string();
        println!("999 -> {err}");
        assert!(err.contains("999"), "どのrefが悪いか言う: {err}");

        // A duplicated element (same text, same href — the Google-btnK shape)
        // still gets an anchor: the candidate pinned to its own position
        let dup2 = text
            .lines()
            .filter(|l| l.contains("重複"))
            .nth(1)
            .and_then(|l| l.strip_prefix('['))
            .and_then(|l| l.split(']').next())
            .and_then(|n| n.parse::<u32>().ok())
            .expect("2つ目の重複リンクのref");
        let rep = b.click(None, &Sel::Ref(dup2), 10_000).unwrap();
        let (kind, v) = rep.anchor.expect("重複でもアンカーが出る");
        println!("dup anchor = {kind} {v}");
        assert_eq!(kind, "xpath");
        assert!(
            v.starts_with('(') && v.ends_with(")[2]"),
            "2つ目の要素は位置ピン留めになる: {v}"
        );

        drop(b);
        std::thread::sleep(Duration::from_millis(600));
    }

    /// A hidden page (bounds 0×0, as during an operate rally showing the AI
    /// tab) has no compositor, so genuine mouse acks never come — the click
    /// must fall back to the synthetic path and still land. Key events and
    /// digest must work hidden as-is.
    ///
    ///   cargo test hidden_page_ref_click -- --ignored --nocapture
    #[test]
    #[ignore]
    fn hidden_page_ref_click_falls_back() {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        std::thread::spawn(move || {
            for req in server.incoming_requests() {
                // mousedown fires only for genuine mouse input (a synthetic
                // el.click() skips it); beforeinput fires only for genuine
                // typing (the native-setter fallback dispatches `input` only).
                // Counting them tells WHICH path the operation really took
                let body = r#"<title>t</title><body>
                  <button id="b" onclick="document.getElementById('log').textContent='clicked'">押す</button>
                  <input id="i" placeholder="名前">
                  <div id="log"></div>
                  <script>
                    window.__ev = { md: 0, bi: 0 };
                    document.getElementById('b').addEventListener('mousedown', () => __ev.md++);
                    document.getElementById('i').addEventListener('beforeinput', () => __ev.bi++);
                  </script>"#;
                let _ = req.respond(
                    tiny_http::Response::from_string(body).with_header(
                        tiny_http::Header::from_bytes(
                            &b"Content-Type"[..],
                            &b"text/html; charset=utf-8"[..],
                        )
                        .unwrap(),
                    ),
                );
            }
        });
        let url = format!("http://127.0.0.1:{port}/");
        let b = Browser::spawn(&url, "SHIKISHA-TERM hidden probe").expect("窓が開かない");
        // A page placed at zero size = hidden (how the app hides pages)
        b.open_child("c", &url, (0, 0, 0, 0), BrowserProfile::new("", true)).unwrap();
        std::thread::sleep(Duration::from_millis(2500));

        let text = b.digest(Some("c"), 20_000).expect("非表示ページのdigestが取れない");
        println!("{text}");
        let ref_of = |needle: &str| -> u32 {
            text.lines()
                .find(|l| l.contains(needle))
                .and_then(|l| l.strip_prefix('['))
                .and_then(|l| l.split(']').next())
                .and_then(|n| n.parse().ok())
                .unwrap_or_else(|| panic!("refが取れない: {needle}"))
        };

        let t0 = std::time::Instant::now();
        assert_eq!(
            b.click(Some("c"), &Sel::Ref(ref_of("押す")), 10_000).unwrap().state,
            Found::Visible,
            "非表示でもクリックは成立する"
        );
        println!("click took {}ms", t0.elapsed().as_millis());
        std::thread::sleep(Duration::from_millis(400));
        let id = b.eval_in(Some("c"), "return document.getElementById('log').textContent;").unwrap();
        assert_eq!(
            b.wait_result(id, Duration::from_secs(10)).unwrap(),
            "\"clicked\"",
            "onclickが発火する"
        );

        assert_eq!(
            b.fill(Some("c"), &Sel::Ref(ref_of("名前")), "俳句", 10_000).unwrap().state,
            Found::Visible
        );
        std::thread::sleep(Duration::from_millis(400));
        let id = b.eval_in(Some("c"), "return document.getElementById('i').value;").unwrap();
        let v = b.wait_result(id, Duration::from_secs(10)).unwrap();
        assert_eq!(v, "\"俳句\"", "非表示でも値は必ず入る");

        // The wake must have made GENUINE input land — not the fallbacks
        let id = b.eval_in(Some("c"), "return JSON.stringify(window.__ev);").unwrap();
        let ev = b.wait_result(id, Duration::from_secs(10)).unwrap();
        println!("genuine-input evidence = {ev}");
        assert!(
            ev.contains("\\\"md\\\":1") || ev.contains("md\":1"),
            "本物マウス (mousedown) が非表示ページに届くはず: {ev}"
        );
        assert!(
            !ev.contains("bi\":0"),
            "本物の打鍵 (beforeinput) が非表示ページに届くはず: {ev}"
        );

        drop(b);
        std::thread::sleep(Duration::from_millis(600));
    }

    /// Auto-wait (the actionability engine) end-to-end:
    /// a click waits for an element the page hasn't built yet, waits out an
    /// animation, and — the replay.lua case — ops fired back-to-back with no
    /// pauses survive a navigation in between, because the outer retry
    /// re-enters the new document.
    ///
    ///   cargo test auto_wait_round_trip -- --ignored --nocapture
    #[test]
    #[ignore]
    fn auto_wait_round_trip() {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        std::thread::spawn(move || {
            for req in server.incoming_requests() {
                let body = if req.url().starts_with("/two") {
                    // The input arrives 600ms late — a client-rendered page
                    r#"<title>two</title><body>
                      <div id="slot"></div>
                      <button id="ok" onclick="document.getElementById('out').textContent =
                        document.getElementById('name').value">OK</button>
                      <div id="out"></div>
                      <script>
                        setTimeout(() => {
                          document.getElementById('slot').innerHTML =
                            '<input id="name" placeholder="なまえ">';
                        }, 600);
                      </script>"#
                } else {
                    // #late appears after 700ms; #move slides for ~500ms first
                    r#"<title>one</title><body>
                      <button id="move" style="transition:margin-left .5s" onclick="this.dataset.hit='1'">動く</button>
                      <div id="slot"></div>
                      <a id="go" href="/two">つぎへ</a>
                      <script>
                        requestAnimationFrame(() => { document.getElementById('move').style.marginLeft = '120px'; });
                        setTimeout(() => {
                          document.getElementById('slot').innerHTML =
                            '<button id="late" onclick="this.dataset.hit=1">遅れて出る</button>';
                        }, 700);
                      </script>"#
                };
                let _ = req.respond(
                    tiny_http::Response::from_string(body).with_header(
                        tiny_http::Header::from_bytes(
                            &b"Content-Type"[..],
                            &b"text/html; charset=utf-8"[..],
                        )
                        .unwrap(),
                    ),
                );
            }
        });
        let url = format!("http://127.0.0.1:{port}/");
        let b = Browser::spawn(&url, "SHIKISHA-TERM auto-wait probe").expect("窓が開かない");

        // Back-to-back, replay-style: no pauses between any of these
        let t0 = std::time::Instant::now();
        let r = b.click(None, &Sel::Css("#late".into()), 10_000).unwrap();
        let waited = t0.elapsed().as_millis();
        assert_eq!(r.state, Found::Visible, "まだ無い要素を待ってクリックできる");
        println!("late click waited {waited}ms");
        assert!(waited >= 500, "700ms後に現れる要素を待ったはず: {waited}ms");

        assert_eq!(
            b.click(None, &Sel::Css("#move".into()), 10_000).unwrap().state,
            Found::Visible,
            "アニメーション中の要素は安定を待ってクリック"
        );

        // Navigate, then immediately act on the next page's late element
        assert_eq!(b.click(None, &Sel::Css("#go".into()), 10_000).unwrap().state, Found::Visible);
        assert_eq!(
            b.fill(None, &Sel::Css("#name".into()), "俳句", 10_000).unwrap().state,
            Found::Visible,
            "遷移直後+遅延生成の入力欄に、待ち無しの連打で書ける"
        );
        assert_eq!(b.click(None, &Sel::Css("#ok".into()), 10_000).unwrap().state, Found::Visible);
        std::thread::sleep(Duration::from_millis(300));
        let id = b.eval("return document.getElementById('out').textContent + ' @ ' + location.pathname;").unwrap();
        let v = b.wait_result(id, Duration::from_secs(10)).unwrap();
        println!("final: {v}");
        assert_eq!(v, "\"俳句 @ /two\"", "連打リプレイが最後まで通る");

        // A truly absent element still says not_found — after the full wait
        let t0 = std::time::Instant::now();
        let r = b.click(None, &Sel::Css("#never".into()), 2_500).unwrap();
        assert_eq!(r.state, Found::NotFound);
        println!("absent verdict after {}ms", t0.elapsed().as_millis());

        drop(b);
        std::thread::sleep(Duration::from_millis(600));
    }

    /// The full task an operator AI is asked to do, walked with the primitives
    /// alone against the live Google: digest → fill the search box by ref →
    /// Enter → digest the results → click the Wikipedia link by ref → land on
    /// ja.wikipedia.org. If this passes, every mechanical link in the chain
    /// (digest quality included) is sound and only the AI's judgment remains.
    ///
    ///   cargo test haiku_task_probe -- --ignored --nocapture
    #[test]
    #[ignore]
    fn haiku_task_probe() {
        let b = Browser::spawn("https://www.google.com/", "SHIKISHA-TERM task probe")
            .expect("窓が開かない");
        std::thread::sleep(Duration::from_millis(1500));

        let ref_of = |text: &str, needle: &str| -> Option<u32> {
            text.lines()
                .find(|l| l.contains(needle))
                .and_then(|l| l.strip_prefix('['))
                .and_then(|l| l.split(']').next())
                .and_then(|n| n.parse().ok())
        };

        // 1. Find and fill the search box
        let d1 = b.digest(None, 20_000).expect("digest 1");
        let q = ref_of(&d1, "combobox").or_else(|| ref_of(&d1, "textbox")).expect("検索窓");
        let rep = b.fill(None, &Sel::Ref(q), "俳句", 10_000).expect("fill");
        println!("fill -> {:?} {:?}", rep.state, rep.echo);
        assert_eq!(rep.state, Found::Visible);

        // 2. Submit with Enter (the key goes to the focused element = the box)
        b.inject(None, Input::Key { named: "enter".into(), ctrl: false, alt: false }).unwrap();
        let url = b.wait_ready(Duration::from_secs(20)).expect("検索結果が来ない");
        println!("results: {url}");
        assert!(url.contains("/search"), "検索結果ページに遷移: {url}");
        std::thread::sleep(Duration::from_millis(1200));

        // 3. Digest the results and click the Wikipedia link by number
        let d2 = b.digest(None, 20_000).expect("digest 2");
        println!("---- results digest ----\n{d2}\n----");
        // Read like a careful operator: the real result link lives under the
        // results-section heading; the same URL quoted inside an AI summary
        // (§AI…) opens a citation panel instead of navigating
        let wiki_links: Vec<&str> = d2
            .lines()
            .filter(|l| {
                l.starts_with('[') && l.contains("link") && l.contains("ja.wikipedia.org/wiki")
            })
            .collect();
        let wiki = wiki_links
            .iter()
            .find(|l| l.contains("§ウェブ検索結果") || l.contains("§検索結果"))
            .or_else(|| wiki_links.iter().find(|l| !l.contains("§AI")))
            .copied()
            .expect("結果セクションのWikipediaリンクがdigestに載る");
        println!("wiki line: {wiki}");
        let r: u32 = wiki
            .strip_prefix('[')
            .and_then(|l| l.split(']').next())
            .and_then(|n| n.parse().ok())
            .unwrap();
        let rep = b.click(None, &Sel::Ref(r), 10_000).expect("click");
        println!("click -> {:?} {:?} anchor={:?}", rep.state, rep.echo, rep.anchor);
        let echo = rep.echo.clone().unwrap_or_default();
        assert!(
            echo.contains("俳句") || echo.to_lowercase().contains("wikipedia"),
            "エコーがWikipediaリンクを名乗る: {echo}"
        );
        let url = b.wait_ready(Duration::from_secs(20)).expect("Wikipediaへ遷移しない");
        println!("landed: {url}");
        assert!(url.contains("ja.wikipedia.org/wiki"), "Wikipediaに着地: {url}");

        drop(b);
        std::thread::sleep(Duration::from_millis(600));
    }

    /// Field probe against the real Google homepage: where exactly does a
    /// ref-click stall? Prints per-step timings instead of asserting, so the
    /// failing CDP call names itself.
    ///
    ///   cargo test google_probe -- --ignored --nocapture
    #[test]
    #[ignore]
    fn google_probe() {
        let b = Browser::spawn("https://www.google.com/", "SHIKISHA-TERM google probe")
            .expect("窓が開かない");
        std::thread::sleep(Duration::from_millis(1500));

        let t0 = std::time::Instant::now();
        let text = b.digest(None, 20_000).expect("digestが取れない");
        println!("digest: {}ms, {} lines\n{text}", t0.elapsed().as_millis(), text.lines().count());

        let ref_of = |needle: &str| -> Option<u32> {
            text.lines()
                .find(|l| l.contains(needle))
                .and_then(|l| l.strip_prefix('['))
                .and_then(|l| l.split(']').next())
                .and_then(|n| n.parse().ok())
        };
        let box_ref = ref_of("combobox")
            .or_else(|| ref_of("textbox"))
            .expect("検索窓が見つからない");
        println!("search box = ref {box_ref}");

        let t0 = std::time::Instant::now();
        let r = b.fill(None, &Sel::Ref(box_ref), "俳句", 10_000);
        println!("fill: {:?} in {}ms", r, t0.elapsed().as_millis());

        std::thread::sleep(Duration::from_millis(800));
        let text2 = b.digest(None, 20_000).expect("2度目のdigestが取れない");
        let btn = text2
            .lines()
            .find(|l| l.contains("button") && l.contains("検索") && !l.contains("画像"))
            .map(str::to_string)
            .expect("検索ボタンが見つからない");
        println!("button line: {btn}");
        let btn_ref: u32 = btn
            .strip_prefix('[')
            .and_then(|l| l.split(']').next())
            .and_then(|n| n.parse().ok())
            .unwrap();

        // What would the replay journal record for this button?
        let oid = b.ref_object(None, btn_ref, 8_000).unwrap();
        println!("button anchor = {:?}", b.element_anchor(None, &oid, 8_000));

        // The same steps click_ref takes, timed one by one
        let backend = b.ref_backend(None, btn_ref).unwrap();
        for (what, method, params) in [
            ("scroll", "DOM.scrollIntoViewIfNeeded", serde_json::json!({"backendNodeId": backend})),
            ("quads", "DOM.getContentQuads", serde_json::json!({"backendNodeId": backend})),
        ] {
            let t0 = std::time::Instant::now();
            let r = b.cdp(None, method, params, 8_000);
            println!("{what}: {}ms ok={}", t0.elapsed().as_millis(), r.is_ok());
        }
        let q = b
            .cdp(None, "DOM.getContentQuads", serde_json::json!({"backendNodeId": backend}), 8_000)
            .unwrap();
        let (x, y) = Browser::quad_center(&q).unwrap();
        for (kind, buttons, clicks) in
            [("mouseMoved", 0, 0), ("mousePressed", 1, 1), ("mouseReleased", 0, 1)]
        {
            let t0 = std::time::Instant::now();
            let r = b.cdp(
                None,
                "Input.dispatchMouseEvent",
                serde_json::json!({"type": kind, "x": x, "y": y,
                                   "button": "left", "buttons": buttons, "clickCount": clicks}),
                8_000,
            );
            println!(
                "{kind}: {}ms ok={} {:?}",
                t0.elapsed().as_millis(),
                r.is_ok(),
                r.err().map(|e| e.to_string())
            );
        }
        std::thread::sleep(Duration::from_millis(2500));
        let id = b.eval("return location.href;").unwrap();
        println!("after click url = {:?}", b.wait_result(id, Duration::from_secs(10)));
        drop(b);
    }
}

/// The window, seen as "something that shows pages".
///
/// Every one of these already existed as an inherent method; the trait is what
/// lets the runtime call them without knowing a window is what answers.
impl shikisha_shared::BrowserHost for Browser {
    fn go(&self, to: Option<&str>, go: Go) -> Result<()> { Browser::go(self, to, go) }
    fn focus(&self, to: Option<&str>) -> Result<()> { Browser::focus(self, to) }
    fn ask_where(&self, to: Option<&str>) -> Result<()> { Browser::ask_where(self, to) }
    fn basic_auth(&self, to: Option<&str>, user: &str, pass: &str) -> Result<()> {
        Browser::basic_auth(self, to, user, pass)
    }
    fn eval_in(&self, to: Option<&str>, js: &str) -> Result<u64> { Browser::eval_in(self, to, js) }
    fn inject(&self, to: Option<&str>, input: Input) -> Result<()> { Browser::inject(self, to, input) }
    fn screencast(&self, to: Option<&str>, on: bool) -> Result<()> { Browser::screencast(self, to, on) }
    fn record(&self, to: Option<&str>, on: bool) -> Result<()> { Browser::record(self, to, on) }

    fn find(&self, to: Option<&str>, sel: &Sel, timeout_ms: u64) -> Result<Found> {
        Browser::find(self, to, sel, timeout_ms)
    }
    fn click(&self, to: Option<&str>, sel: &Sel, timeout_ms: u64) -> Result<OpReport> {
        Browser::click(self, to, sel, timeout_ms)
    }
    fn fill(&self, to: Option<&str>, sel: &Sel, value: &str, timeout_ms: u64) -> Result<OpReport> {
        Browser::fill(self, to, sel, value, timeout_ms)
    }
    fn text(&self, to: Option<&str>, sel: &Sel, timeout_ms: u64) -> Result<Option<String>> {
        Browser::text(self, to, sel, timeout_ms)
    }
    fn href(&self, to: Option<&str>, timeout_ms: u64) -> Result<String> { Browser::href(self, to, timeout_ms) }
    fn html(&self, to: Option<&str>, timeout_ms: u64) -> Result<String> { Browser::html(self, to, timeout_ms) }
    fn digest(&self, to: Option<&str>, timeout_ms: u64) -> Result<String> { Browser::digest(self, to, timeout_ms) }
    fn snapshot(&self, to: Option<&str>, timeout_ms: u64) -> Result<Vec<u8>> { Browser::snapshot(self, to, timeout_ms) }

    fn cookies_out(&self, to: Option<&str>, timeout_ms: u64) -> Result<serde_json::Value> {
        Browser::cookies_out(self, to, timeout_ms)
    }
    fn cookies_in(&self, to: Option<&str>, cookies: &serde_json::Value, timeout_ms: u64) -> Result<()> {
        Browser::cookies_in(self, to, cookies, timeout_ms)
    }
    fn storage_out(&self, to: Option<&str>, timeout_ms: u64) -> Result<serde_json::Value> {
        Browser::storage_out(self, to, timeout_ms)
    }
    fn storage_in(&self, to: Option<&str>, items: &serde_json::Value, timeout_ms: u64) -> Result<()> {
        Browser::storage_in(self, to, items, timeout_ms)
    }
    fn fetch(&self, to: Option<&str>, url: &str, opts: &serde_json::Value, timeout_ms: u64) -> Result<String> {
        Browser::fetch(self, to, url, opts, timeout_ms)
    }

    fn open_child(&self, name: &str, url: &str, rect: (i32, i32, i32, i32), profile: BrowserProfile) -> Result<()> {
        Browser::open_child(self, name, url, rect, profile)
    }
    fn child_bounds(&self, name: &str, rect: (i32, i32, i32, i32)) -> Result<()> {
        Browser::child_bounds(self, name, rect)
    }
    fn close_child(&self, name: &str) -> Result<()> { Browser::close_child(self, name) }
    fn trust(&self, url: &str) -> Result<()> { Browser::trust(self, url) }
    fn record_all_off(&self) { Browser::record_all_off(self) }
}

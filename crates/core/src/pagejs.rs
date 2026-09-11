//! The script every page is given, before the page itself runs.
//!
//! It is the page-side half of `browser_*`: the automation calls into these
//! helpers, so the half that runs inside the document is written in JavaScript
//! and the half that decides anything is written in Rust.
//!
//! It lives here rather than beside either host because both hosts inject the
//! very same text -- the window into its WebView2, the server into its headless
//! Chromium. Two copies would agree on the day they were written and drift
//! afterwards, and the drift would show up as automation that works in one
//! place and quietly does something else in the other.
//!
//! The one thing a host has to supply is where a report goes:
//! `window.__shikisha_post(json)` is defined before this runs -- the window
//! points it at its own channel, the server at a protocol binding.

/// The whole of it, as injected.
pub const AUTOMATION: &str = r##"
(function () {
  if (window.__shikisha) return;
  const send = (o) => window.__shikisha_post(JSON.stringify(o));

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

//! What can be done to a page, written once for both browsers.
//!
//! Every operation below was already expressed in two moves: call one method
//! of the DevTools protocol, or run one piece of JavaScript in the page and
//! wait for what it returns. Neither move cares which browser is on the other
//! end, so neither does anything here -- the window hands these its WebView2,
//! the server hands them its headless Chromium, and a script cannot tell which
//! one answered.
//!
//! That is the point of it. A selector that finds a button in the window has to
//! find the same button on the server, and the only way to be sure of that is
//! for the same code to do the finding.

use shikisha_shared::{Found, OpReport, Sel};

/// A browser that answers the protocol, whatever it is and however it is
/// reached.
///
/// `to` names the page throughout: `None` is the one in front.
pub trait Speaks {
    /// Call one method on a page and wait for its result, parsed
    fn cdp(
        &self,
        to: Option<&str>,
        method: &str,
        params: serde_json::Value,
        timeout_ms: u64,
    ) -> anyhow::Result<serde_json::Value>;

    /// Run a body of JavaScript in the page and wait for the value it returns,
    /// as JSON text. A promise is awaited before it counts as returned
    fn eval(&self, to: Option<&str>, js: &str, timeout_ms: u64) -> anyhow::Result<String>;

    /// Where each page's latest digest refs are remembered.
    ///
    /// A `{ref=N}` is a number in the last digest taken of that page, and the
    /// numbers die with the document they were taken from
    fn refs(&self) -> &std::sync::Mutex<std::collections::HashMap<Option<String>, Vec<i64>>>;

    /// Keep a page's compositor running while genuine input is dispatched.
    ///
    /// Only the window needs this: a page it has hidden stops producing
    /// frames, and mouse input is the one kind that waits for one. A browser
    /// with no screen at all composites regardless
    fn wake(&self, to: Option<&str>, on: bool) {
        let _ = (to, on);
    }
}

/// Build a JS function call.
///
/// **Arguments must always go through here.** Everything is serialized with
/// `serde_json`, so quotes and newlines survive intact and the value passed in
/// is never interpreted as code. Even AI output, or text read straight off a
/// page, arrives as a plain value
pub fn call_js(func: &str, args: &[serde_json::Value]) -> String {
    let list: Vec<String> = args.iter().map(std::string::ToString::to_string).collect();
    format!("return window.{func}({});", list.join(","))
}

/// Call JS once and wait for the result
pub fn call(
    s: &dyn Speaks,
    to: Option<&str>,
    func: &str,
    args: &[serde_json::Value],
    timeout_ms: u64,
) -> anyhow::Result<String> {
    s.eval(to, &call_js(func, args), timeout_ms)
}

/// Where that element currently is
pub fn find(s: &dyn Speaks, to: Option<&str>, sel: &Sel, timeout_ms: u64) -> anyhow::Result<Found> {
    if let Sel::Ref(r) = sel {
        return find_ref(s, to, *r, timeout_ms);
    }
    Ok(Found::parse(&call(s, 
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
pub fn href(s: &dyn Speaks, to: Option<&str>, timeout_ms: u64) -> anyhow::Result<String> {
    let v = call(s, to, "__shikisha_href", &[], timeout_ms)?;
    Ok(serde_json::from_str::<String>(&v).unwrap_or_default())
}

pub fn text(s: &dyn Speaks, to: Option<&str>, sel: &Sel, timeout_ms: u64) -> anyhow::Result<Option<String>> {
    if let Sel::Ref(r) = sel {
        return text_ref(s, to, *r, timeout_ms);
    }
    let v = call(s, to, "__shikisha_text", &[sel.json()], timeout_ms)?;
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
pub fn act_with_wait(
    s: &dyn Speaks,
    to: Option<&str>,
    func: &str,
    mut args: Vec<serde_json::Value>,
    timeout_ms: u64,
) -> anyhow::Result<Found> {
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
        let res = call(s, to, func, &args, slice + CUSHION_MS);
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
pub fn click(s: &dyn Speaks, to: Option<&str>, sel: &Sel, timeout_ms: u64) -> anyhow::Result<OpReport> {
    if let Sel::Ref(r) = sel {
        return click_ref(s, to, *r, timeout_ms);
    }
    // Woken like a click by ref. The wait for the element to settle counts
    // animation frames, and a page the window has hidden draws none: without
    // this, every click by selector on a page not in front waited out its
    // whole deadline and came back with no answer
    s.wake(to, true);
    let out = act_with_wait(s, to, "__shikisha_click", vec![sel.json()], timeout_ms);
    s.wake(to, false);
    Ok(OpReport::bare(out?))
}

/// Put a value into an input field. A `{ref=N}` types genuine key events
/// and reports which field was written; selectors keep the value-setter
/// route. Both paths auto-wait (see `act_with_wait`)
pub fn fill(
    s: &dyn Speaks,
    to: Option<&str>,
    sel: &Sel,
    value: &str,
    timeout_ms: u64,
) -> anyhow::Result<OpReport> {
    if let Sel::Ref(r) = sel {
        return fill_ref(s, to, *r, value, timeout_ms);
    }
    // The same wake, for the same frames (see `click`)
    s.wake(to, true);
    let out = act_with_wait(
        s,
        to,
        "__shikisha_fill",
        vec![sel.json(), serde_json::Value::String(value.to_string())],
        timeout_ms,
    );
    s.wake(to, false);
    Ok(OpReport::bare(out?))
}

/// The full parsed HTML
pub fn html(s: &dyn Speaks, to: Option<&str>, timeout_ms: u64) -> anyhow::Result<String> {
    let v = call(s, to, "__shikisha_html", &[], timeout_ms)?;
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
pub fn cookies_out(s: &dyn Speaks, to: Option<&str>, timeout_ms: u64) -> anyhow::Result<serde_json::Value> {
    let v = s.cdp(to, "Network.getAllCookies", serde_json::json!({}), timeout_ms)?;
    Ok(v.get("cookies").cloned().unwrap_or(serde_json::Value::Array(vec![])))
}

/// A picture of the page as it looks right now, as PNG bytes.
///
/// Taken by the browser itself through the devtools protocol, so it is what
/// a person would see, not a re-render of the HTML. For a rally to keep a
/// visual record of what it did, or for a person to glance at where an
/// agent got to without switching to the tab
pub fn snapshot(s: &dyn Speaks, to: Option<&str>, timeout_ms: u64) -> anyhow::Result<Vec<u8>> {
    use base64::Engine as _;
    let v = s.cdp(
        to,
        "Page.captureScreenshot",
        serde_json::json!({ "format": "png", "captureBeyondViewport": false }),
        timeout_ms,
    )?;
    let data = v
        .get("data")
        .and_then(|d| d.as_str())
        .ok_or_else(|| anyhow::anyhow!(crate::i18n::t("err.browser.no_snapshot")))?;
    base64::engine::general_purpose::STANDARD
        .decode(data.as_bytes())
        .map_err(|e| anyhow::anyhow!(crate::i18n::tp("err.browser.no_snapshot_decode", &[("e", &e.to_string())])))
}

/// Put a set of cookies back into this page's profile.
///
/// The same shape that came out. Set against the live profile, so a page
/// that reloads afterwards is simply logged in -- there was never a moment
/// where our code decided what "logged in" meant
pub fn cookies_in(
    s: &dyn Speaks,
    to: Option<&str>,
    cookies: &serde_json::Value,
    timeout_ms: u64,
) -> anyhow::Result<()> {
    s.cdp(
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
pub fn storage_out(s: &dyn Speaks, to: Option<&str>, timeout_ms: u64) -> anyhow::Result<serde_json::Value> {
    let v = s.cdp(
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
    s: &dyn Speaks,
    to: Option<&str>,
    items: &serde_json::Value,
    timeout_ms: u64,
) -> anyhow::Result<()> {
    let payload = serde_json::to_string(items).unwrap_or_else(|_| "[]".into());
    let expr = format!(
        "(()=>{{try{{for(const [k,v] of {payload}){{localStorage.setItem(k,v)}}return true}}catch(e){{return false}}}})()"
    );
    s.cdp(
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
    s: &dyn Speaks,
    to: Option<&str>,
    url: &str,
    opts: &serde_json::Value,
    timeout_ms: u64,
) -> anyhow::Result<String> {
    call(s, 
        to,
        "__shikisha_fetch",
        &[serde_json::Value::String(url.to_string()), opts.clone()],
        timeout_ms,
    )
}

/// Distill the page into its operable elements (see `crate::digest`), and
/// remember the ref-number → backendNodeId mapping for `{ref=N}` calls
pub fn digest(s: &dyn Speaks, to: Option<&str>, timeout_ms: u64) -> anyhow::Result<String> {
    let metrics = s.cdp(to, "Page.getLayoutMetrics", serde_json::json!({}), timeout_ms)?;
    let snap = s.cdp(
        to,
        "DOMSnapshot.captureSnapshot",
        serde_json::json!({ "computedStyles": ["cursor"] }),
        timeout_ms,
    )?;
    // Roles and accessible names, as the browser itself computed them
    let ax = s.cdp(
        to,
        "Accessibility.getFullAXTree",
        serde_json::json!({}),
        timeout_ms,
    )?;
    let d = crate::digest::build(&ax, &snap, &metrics);
    s.refs()
        .lock()
        .unwrap()
        .insert(to.map(str::to_string), d.refs);
    Ok(d.text)
}

/// Resolve `{ref=N}` against the latest digest of that page
pub fn ref_backend(s: &dyn Speaks, to: Option<&str>, r: u32) -> anyhow::Result<i64> {
    let map = s.refs().lock().unwrap();
    let refs = map
        .get(&to.map(str::to_string))
        .ok_or_else(|| anyhow::anyhow!(crate::i18n::t("err.browser.ref_no_digest")))?;
    (r as usize)
        .checked_sub(1)
        .and_then(|i| refs.get(i))
        .copied()
        .ok_or_else(|| {
            anyhow::anyhow!(crate::i18n::tp(
                "err.browser.ref_unknown",
                &[("ref", &r.to_string()), ("max", &refs.len().to_string())]
            ))
        })
}

/// Word a CDP failure on a ref as what it almost always is: the element
/// (or the whole document) is gone since the digest was taken
pub fn ref_stale(r: u32, e: anyhow::Error) -> anyhow::Error {
    anyhow::anyhow!(crate::i18n::tp(
        "err.browser.ref_stale",
        &[("ref", &r.to_string()), ("e", &e.to_string())]
    ))
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
pub fn click_ref(s: &dyn Speaks, to: Option<&str>, r: u32, timeout_ms: u64) -> anyhow::Result<OpReport> {
    s.wake(to, true);
    let out = click_ref_inner(s, to, r, timeout_ms);
    s.wake(to, false);
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
pub fn element_anchor(
    s: &dyn Speaks,
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
    let v = s
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
pub fn ref_ready(
    s: &dyn Speaks,
    to: Option<&str>,
    oid: &str,
    hit: bool,
    deadline_ms: u64,
    timeout_ms: u64,
) -> anyhow::Result<(bool, f64, f64, String)> {
    const READY: &str = r#"function (deadline, hit) {
        return window.__shikisha_ready(this, { deadline: deadline, enabled: true, hit: hit })
            .then((r) => JSON.stringify(r));
    }"#;
    let v = s.cdp(
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
pub fn click_desc(s: &dyn Speaks, to: Option<&str>, oid: &str, timeout_ms: u64) -> Option<String> {
    const DESC: &str = r#"function () {
        const t = this.innerText || this.value || this.getAttribute("aria-label")
               || this.getAttribute("alt") || "";
        return this.tagName.toLowerCase() + " 「"
             + Array.from(String(t).replace(/\s+/g, " ").trim()).slice(0, 60).join("") + "」";
    }"#;
    s.cdp(
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

pub fn click_ref_inner(s: &dyn Speaks, to: Option<&str>, r: u32, timeout_ms: u64) -> anyhow::Result<OpReport> {
    let oid = ref_object(s, to, r, timeout_ms)?;
    // Auto-wait for visible/enabled/stable and a clean hit target
    // (scrolling happens inside, cycling alignments per retry)
    let deadline = timeout_ms.saturating_sub(1_500).max(1_000);
    let (ok, x, y, why) = ref_ready(s, to, &oid, true, deadline, timeout_ms)
        .map_err(|e| ref_stale(r, e))?;
    let desc = click_desc(s, to, &oid, timeout_ms);
    let anchor = element_anchor(s, to, &oid, timeout_ms);

    let synthetic_click = || {
        s.cdp(
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
            return Err(anyhow::anyhow!(crate::i18n::tp(
                "err.browser.ref_stale",
                &[("ref", &r.to_string()), ("e", "detached")]
            )));
        }
        // Never actionable within the deadline (covered / unstable /
        // hidden): honor the ref with the element's own click() — the
        // pre-auto-wait behavior — and record why
        crate::append_hook_log(&format!(
            "ref click {r}: not actionable ({why}) — using the element's own click()"
        ));
        synthetic_click()?;
        return Ok(OpReport { state: Found::Visible, echo: desc, anchor });
    }

    const ACK_MS: u64 = 1_500;
    let probe = s.cdp(
        to,
        "Input.dispatchMouseEvent",
        serde_json::json!({ "type": "mouseMoved", "x": x, "y": y,
                           "button": "left", "buttons": 0, "clickCount": 0 }),
        ACK_MS,
    );
    if probe.is_ok() {
        for (kind, buttons) in [("mousePressed", 1), ("mouseReleased", 0)] {
            s.cdp(
                to,
                "Input.dispatchMouseEvent",
                serde_json::json!({ "type": kind, "x": x, "y": y,
                                   "button": "left", "buttons": buttons, "clickCount": 1 }),
                timeout_ms,
            )?;
        }
        return Ok(OpReport { state: Found::Visible, echo: desc, anchor });
    }
    crate::append_hook_log(&format!(
        "ref click {r}: no input ack — falling back to synthetic click"
    ));
    synthetic_click()?;
    Ok(OpReport { state: Found::Visible, echo: desc, anchor })
}

/// Resolve a ref to a JS object handle (for focus/read, not for input)
pub fn ref_object(s: &dyn Speaks, to: Option<&str>, r: u32, timeout_ms: u64) -> anyhow::Result<String> {
    let b = ref_backend(s, to, r)?;
    let node = s
        .cdp(
            to,
            "DOM.resolveNode",
            serde_json::json!({ "backendNodeId": b }),
            timeout_ms,
        )
        .map_err(|e| ref_stale(r, e))?;
    node.get("object")
        .and_then(|o| o.get("objectId"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| {
            anyhow::anyhow!(crate::i18n::tp(
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
pub fn fill_ref(s: &dyn Speaks, to: Option<&str>, r: u32, value: &str, timeout_ms: u64) -> anyhow::Result<OpReport> {
    s.wake(to, true);
    let out = fill_ref_inner(s, to, r, value, timeout_ms);
    s.wake(to, false);
    out
}

/// The field's identity for the echo: tag plus its label-ish attribute.
/// Deliberately attribute-only — the field's value never appears here
pub fn field_desc(s: &dyn Speaks, to: Option<&str>, oid: &str, timeout_ms: u64) -> Option<String> {
    const DESC: &str = r#"function () {
        const t = this.getAttribute("placeholder") || this.getAttribute("aria-label")
               || this.getAttribute("name") || this.id || "";
        return this.tagName.toLowerCase()
             + (t ? " 「" + Array.from(String(t)).slice(0, 40).join("") + "」" : "");
    }"#;
    s.cdp(
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

pub fn fill_ref_inner(s: &dyn Speaks, to: Option<&str>, r: u32, value: &str, timeout_ms: u64) -> anyhow::Result<OpReport> {
    // Make the page believe it has focus even when its window doesn't
    // (hidden or unfocused pages otherwise drop keystrokes). Sticky per
    // session and harmless when visible, so arming is idempotent
    let _ = s.cdp(
        to,
        "Emulation.setFocusEmulationEnabled",
        serde_json::json!({ "enabled": true }),
        timeout_ms,
    );
    let oid = ref_object(s, to, r, timeout_ms)?;
    // Auto-wait for visible/enabled/stable (scrolls into view inside).
    // A field that never settles degrades to acting anyway — the value
    // write is verified afterwards either way
    let deadline = timeout_ms.saturating_sub(1_500).max(1_000);
    let (ok, _, _, why) = ref_ready(s, to, &oid, false, deadline, timeout_ms)
        .map_err(|e| ref_stale(r, e))?;
    if !ok && why == "not_found" {
        return Err(anyhow::anyhow!(crate::i18n::tp(
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
    s.cdp(
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
        s.cdp(
            to,
            "Runtime.callFunctionOn",
            serde_json::json!({ "objectId": oid, "functionDeclaration": SET_VALUE,
                               "arguments": [{ "value": value }],
                               "returnByValue": true }),
            timeout_ms,
        )
        .map(|_| ())
    };
    let desc = field_desc(s, to, &oid, timeout_ms);
    let anchor = element_anchor(s, to, &oid, timeout_ms);
    if value.is_empty() {
        set_native()?;
        return Ok(OpReport { state: Found::Visible, echo: desc, anchor });
    }
    for ch in value.chars() {
        s.cdp(
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
    let got = s
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
        crate::append_hook_log(&format!(
            "ref fill {r}: keystrokes didn't land (page hidden?) — falling back to native setter"
        ));
        set_native()?;
    }
    Ok(OpReport { state: Found::Visible, echo: desc, anchor })
}

/// Where a digest ref currently is, in the same three-state vocabulary
/// selectors use: gone = `not_found`, outside the viewport = `off_screen`
pub fn find_ref(s: &dyn Speaks, to: Option<&str>, r: u32, timeout_ms: u64) -> anyhow::Result<Found> {
    let b = ref_backend(s, to, r)?;
    let Ok(q) = s.cdp(
        to,
        "DOM.getContentQuads",
        serde_json::json!({ "backendNodeId": b }),
        timeout_ms,
    ) else {
        return Ok(Found::NotFound);
    };
    let Some((x, y)) = crate::cdp::quad_center(&q) else {
        return Ok(Found::NotFound);
    };
    let m = s.cdp(to, "Page.getLayoutMetrics", serde_json::json!({}), timeout_ms)?;
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
pub fn text_ref(s: &dyn Speaks, to: Option<&str>, r: u32, timeout_ms: u64) -> anyhow::Result<Option<String>> {
    let oid = ref_object(s, to, r, timeout_ms)?;
    const READ: &str = r#"function () {
        return this.value !== undefined ? String(this.value)
             : (this.innerText || this.textContent || "");
    }"#;
    let v = s.cdp(
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


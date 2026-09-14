//! The tools that start by taking a picture: frame a part of it, then do
//! something with that part.
//!
//! One page, whoever opens it. The window opens it over a picture of its own
//! screen, taken the moment before; a phone opens it over a picture the person
//! chooses, because a web page on a phone has no way to take one. Everything
//! after the picture -- framing it, reading a colour out of it -- is this page,
//! so it behaves the same from either end and is written once.
//!
//! What happens with the answer depends on where the tool was opened from, and
//! the page is told rather than guessing: from the left bar it is offered to
//! the clipboard or to a file.
//!
//! Two of the tools read the framed part with the assistant AI. That is the
//! one thing here that leaves the machine, so it is the one thing that asks
//! first: a desk agrees to it once, for that AI, and until it has the picture
//! does not leave the page. The questions the page asks are answered in one
//! place, [`answer`], whichever end the page is open on.

/// The tools this page can run, in the order they are offered.
///
/// The board's menu is built from this list, so a tool that is not here is not
/// offered anywhere -- a button for something this page cannot do would be a
/// button that does nothing
pub const TOOLS: &[&str] = &["text", "noun", "color", "edit"];

mod edit;

/// The tools that hand the framed part to the assistant AI.
pub const AI_TOOLS: &[&str] = &["text", "noun"];

/// The most a framed picture may weigh on its way to an AI. A whole 4K screen
/// as PNG is a few megabytes; this is room for that and no more
const MAX_PICTURE: usize = 24 << 20;

/// The most names the noun tool gives back
const MAX_NOUNS: usize = 5;

/// How many times an AI is asked before its answer is given up on. An AI told
/// to answer in JSON and nothing else still, now and then, says something
/// first; asking again is cheaper than showing the person that
const ATTEMPTS: usize = 3;

/// The shape an AI is asked to answer a tool in, as a JSON Schema.
///
/// The answer has a field of its own for anything the AI wants to add. Told
/// only "no explanations", an AI that has something to say says it inside the
/// answer -- measured 2026-09-14, a reading that began "the text in the image
/// is as follows" -- and a place to put it is what keeps it out.
///
/// Every field is required, the remark included (empty when there is none):
/// Codex CLI's service refuses a shape with any field left optional
pub fn shape_of(tool: &str) -> Option<serde_json::Value> {
    use serde_json::json;
    let note = json!({"type": "string", "description": "Anything to add, or empty. Never part of the answer"});
    match tool {
        "text" => Some(json!({
            "type": "object",
            "properties": {
                "text": {"type": "string", "description": "The characters in the image, exactly, and nothing else"},
                "note": note,
            },
            "required": ["text", "note"],
            "additionalProperties": false,
        })),
        "noun" => Some(json!({
            "type": "object",
            "properties": {
                "nouns": {
                    "type": "array",
                    "items": {"type": "string"},
                    "maxItems": MAX_NOUNS,
                    "description": "Nouns for what the image shows, the best fit first",
                },
                "note": note,
            },
            "required": ["nouns", "note"],
            "additionalProperties": false,
        })),
        _ => None,
    }
}

/// What a tool asks the AI, in the language the app is shown in.
pub fn prompt_for(tool: &str) -> String {
    prompt_in(tool, &crate::i18n::lang())
}

/// What a tool asks the AI, for the language `code` (a BCP 47 tag such as
/// "en", "ja", "es").
///
/// The language the answer is to be written in is not a word inside the
/// translation. "Write the nouns in English", run through a translator for
/// Spanish, still says English -- nothing about it looks wrong to translate
/// word for word. The prompt holds `{lang}` instead, like every other blank
/// in the word lists, and the code is filled in here
fn prompt_in(tool: &str, code: &str) -> String {
    crate::i18n::tp(&format!("snip.ai.{tool}.prompt"), &[("lang", code)])
}

/// Where a question about sending a picture stands, before anything is sent.
#[derive(Debug, PartialEq)]
enum Gate<'a> {
    /// Agreed, for this AI: a picture may go to it
    Ready { name: &'a str, label: &'a str },
    /// Not agreed for this AI -- never agreed, or agreed for a different one
    Consent { name: &'a str, label: &'a str },
    /// An AI is chosen that is not known to read pictures
    Unsupported { label: &'a str },
    /// No assistant AI is installed, or the one chosen is not
    NoAssistant,
    /// The desk the question came from is not in the settings
    NoDesk,
}

/// Decide, from the assistant AI that would answer and what the desk agreed
/// to, whether a picture may go. The desk comes first: a question from a desk
/// that is not there has nobody to agree
fn gate<'a>(
    assistant: Option<(&'a str, &'a str)>,
    desk_agreed: Option<Option<&str>>,
    reads: impl Fn(&str) -> bool,
) -> Gate<'a> {
    let Some(agreed) = desk_agreed else {
        return Gate::NoDesk;
    };
    let Some((name, label)) = assistant else {
        return Gate::NoAssistant;
    };
    if !reads(name) {
        return Gate::Unsupported { label };
    }
    match agreed == Some(name) {
        true => Gate::Ready { name, label },
        false => Gate::Consent { name, label },
    }
}

/// Answer one question from the tool page, for the desk `desk_id`.
///
/// `{"do":"check"}` -- may a picture go, and to which AI.
/// `{"do":"agree"}` -- the person ticked the box: record it on the desk, for
/// the AI that would answer now.
/// `{"do":"ask","tool":..,"png":<base64>}` -- read the picture. Checked again
/// here, not trusted from the check before it: the settings can change in
/// between, and this is the call that sends.
///
/// Slow (the AI is started and waited for), so called from a thread of its
/// own. The page's `id` comes back with the answer, so an answer meant for an
/// earlier question is never taken for this one
pub fn answer(msg: &serde_json::Value, desk_id: &str) -> serde_json::Value {
    let mut out = answer_inner(msg, desk_id);
    if let (Some(o), Some(id)) = (out.as_object_mut(), msg.get("id")) {
        o.insert("id".into(), id.clone());
    }
    out
}

fn answer_inner(msg: &serde_json::Value, desk_id: &str) -> serde_json::Value {
    use serde_json::json;
    let cfg = crate::config::load();
    let chosen = cfg
        .as_ref()
        .and_then(|c| c.ai_engine.clone())
        .filter(|s| !s.trim().is_empty());
    let assistant = crate::webui::assistant_ai(chosen.as_deref());
    let desks = cfg.map(|c| c.resolve_desks().0).unwrap_or_default();
    let desk = desks.iter().find(|d| !desk_id.is_empty() && d.id == desk_id);
    let decided = gate(
        assistant,
        desk.map(|d| d.send_pictures_to.as_deref()),
        crate::webui::reads_pictures,
    );
    let desk_name = desk.map(|d| d.name.clone()).unwrap_or_default();
    let refused = |g: &Gate| match g {
        Gate::Consent { label, .. } => json!({"state": "consent", "by": label, "desk": desk_name}),
        Gate::Unsupported { label } => json!({"state": "unsupported", "by": label}),
        Gate::NoAssistant => json!({"state": "no_assistant", "chosen": chosen.as_deref().and_then(crate::webui::assistant_label).unwrap_or_default()}),
        Gate::NoDesk => json!({"state": "no_desk"}),
        Gate::Ready { label, .. } => json!({"state": "ready", "by": label}),
    };
    match msg.get("do").and_then(|d| d.as_str()) {
        Some("check") => refused(&decided),
        Some("agree") => match decided {
            Gate::Ready { label, .. } => json!({"state": "ready", "by": label}),
            Gate::Consent { name, label } => {
                if crate::config::save_desk_setting(desk_id, "send_pictures_to", Some(json!(name))) {
                    json!({"state": "ready", "by": label})
                } else {
                    json!({"state": "failed", "by": label, "error": crate::i18n::t("snip.ai.not_saved")})
                }
            }
            other => refused(&other),
        },
        Some("ask") => {
            let Gate::Ready { name, label } = decided else {
                return refused(&decided);
            };
            let tool = msg.get("tool").and_then(|t| t.as_str()).unwrap_or_default();
            if !AI_TOOLS.contains(&tool) {
                return json!({"state": "failed", "by": label, "error": crate::i18n::t("snip.ai.no_picture")});
            }
            let Some(png) = picture_of(msg.get("png").and_then(|p| p.as_str()).unwrap_or_default()) else {
                return json!({"state": "failed", "by": label, "error": crate::i18n::t("snip.ai.no_picture")});
            };
            let Some(shape) = shape_of(tool) else {
                return json!({"state": "failed", "by": label, "error": crate::i18n::t("snip.ai.no_picture")});
            };
            let shape = shape.to_string();
            let asked = prompt_for(tool);
            for attempt in 0..ATTEMPTS {
                // Asked again, it is told why: the answer before was not the shape
                let prompt = match attempt {
                    0 => asked.clone(),
                    _ => format!("{asked}\n\n{}", crate::i18n::t("snip.ai.retry")),
                };
                match crate::webui::ask_about_picture(name, &prompt, &png, &shape) {
                    Ok(said) => {
                        if let Some(mut read) = read_reply(tool, &said) {
                            read["state"] = json!("answer");
                            read["by"] = json!(label);
                            return read;
                        }
                        crate::append_hook_log(&format!(
                            "snip: {name} answered {tool} out of shape (attempt {})",
                            attempt + 1
                        ));
                    }
                    // The AI did not run or did not finish. Asking again would
                    // be waiting the same way twice
                    Err(e) => return json!({"state": "failed", "by": label, "error": format!("{e:#}")}),
                }
            }
            json!({"state": "failed", "by": label,
                   "error": crate::i18n::tp("snip.ai.unreadable", &[("n", &ATTEMPTS.to_string())])})
        }
        _ => json!({"state": "failed", "error": crate::i18n::t("snip.ai.no_picture")}),
    }
}

/// The picture the page sent, if it is one: base64 of a PNG, within the size
/// an AI is handed
fn picture_of(b64: &str) -> Option<Vec<u8>> {
    use base64::Engine as _;
    if b64.is_empty() || b64.len() > MAX_PICTURE / 3 * 4 + 4 {
        return None;
    }
    let bytes = base64::engine::general_purpose::STANDARD.decode(b64.as_bytes()).ok()?;
    bytes.starts_with(b"\x89PNG\r\n\x1a\n").then_some(bytes)
}

/// An AI's answer read as the tool's shape: `{"text": ..}` or `{"lines": [..]}`
/// for the page, or nothing when it is not that shape.
///
/// The JSON is looked for, not trusted to be all there is: the AI that has no
/// way to hold its answer to a shape may still put a word before it or a code
/// fence around it, and the object inside is the answer all the same
fn read_reply(tool: &str, said: &str) -> Option<serde_json::Value> {
    let v = json_in(said)?;
    match tool {
        "text" => {
            let text = v.get("text")?.as_str()?;
            Some(serde_json::json!({"text": text.trim_matches(['\n', '\r'])}))
        }
        "noun" => {
            let mut lines: Vec<String> = Vec::new();
            for item in v.get("nouns")?.as_array()? {
                let w = bare_noun(item.as_str()?);
                if !w.is_empty() && !lines.contains(&w) {
                    lines.push(w);
                }
                if lines.len() == MAX_NOUNS {
                    break;
                }
            }
            Some(serde_json::json!({"lines": lines}))
        }
        _ => None,
    }
}

/// The first JSON object in what an AI printed: all of it, the inside of a
/// code fence, or the span from the first `{` to the last `}`
fn json_in(said: &str) -> Option<serde_json::Value> {
    let t = said.trim();
    let object = |s: &str| serde_json::from_str::<serde_json::Value>(s.trim()).ok().filter(|v| v.is_object());
    if let Some(v) = object(t) {
        return Some(v);
    }
    let (from, to) = (t.find('{')?, t.rfind('}')?);
    (from < to).then(|| object(&t[from..=to])).flatten()
}

/// One noun, without the list marks or the full stop an AI may still give it
fn bare_noun(item: &str) -> String {
    let mut w = item.trim().trim_start_matches(['-', '*', '•', '・', ' ', '\t']);
    let digits = w.find(|c: char| !c.is_ascii_digit()).unwrap_or(0);
    if digits > 0 && w[digits..].starts_with(['.', ')', '、']) {
        w = w[digits + 1..].trim_start();
    }
    w.trim_end_matches(['。', '.', '、', ',']).trim().to_string()
}

/// The seconds a person can choose to wait before the picture is taken.
///
/// The wait is how something behind this program gets into the picture: the
/// program is in front when its button is pressed, and hiding it instead would
/// make it impossible to take a picture of the program itself
pub const WAITS: &[u8] = &[0, 3, 5, 10];

/// The page, in the colours and the language the app is in.
pub fn page() -> String {
    crate::i18n::render(&crate::webui::themed(assembled()))
        .replace("__DICT__", &crate::i18n::dict_json())
        .replace(
            "__TOOLS__",
            &serde_json::to_string(TOOLS).unwrap_or_else(|_| "[]".into()),
        )
}

/// The page with the editor laid into it (see `edit.rs`)
fn assembled() -> String {
    PAGE.replace("/*__EDIT_CSS__*/", edit::CSS)
        .replace("<!--__EDIT_HTML__-->", edit::HTML)
        .replace("<!--__EDIT_JS__-->", edit::JS)
}

const PAGE: &str = r##"<!doctype html>
<html lang="{{__lang__}}"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1, maximum-scale=1, user-scalable=no">
<title>{{snip.title}}</title>
<style>
 :root {
   {{THEME}}
   color-scheme: {{SCHEME}};
   /* The same steps the board is built on (shell.rs), so a control here is the
      height and the roundness a control is everywhere else */
   --s1:4px; --s2:8px; --s3:12px; --s4:16px; --s5:20px; --s6:24px;
   --r-ctl:6px; --r-card:10px; --r-chip:4px;
 }
 * { box-sizing:border-box; }
 html, body { margin:0; height:100%; overflow:hidden; }
 body { background:var(--bg); color:var(--text); font-size:14px; line-height:1.5;
   font-family:system-ui,"Segoe UI","Yu Gothic UI","Hiragino Sans",sans-serif;
   -webkit-user-select:none; user-select:none; touch-action:none; }
 .mono { font-family:ui-monospace,Consolas,"Courier New",monospace; }
 button { font:inherit; font-size:13px; height:32px; padding:0 var(--s3); border-radius:var(--r-ctl);
   border:1px solid var(--line); background:var(--panel2); color:var(--text); cursor:pointer; }
 button:hover { border-color:var(--accent); }
 button.primary { background:var(--accent); border-color:var(--accent); color:var(--bg); font-weight:600; }
 [hidden] { display:none !important; }

 /* Waiting: a small card in a corner of the screen, which the picture leaves out */
 #wait { position:fixed; inset:0; display:flex; align-items:center; justify-content:center; gap:var(--s3);
   background:var(--panel); border:1px solid var(--brand); border-radius:var(--r-card); padding:0 var(--s4); }
 #wait .n { font-size:26px; font-weight:700; color:var(--brand); min-width:1.2em; text-align:center; }
 #wait .say { font-size:13px; color:var(--text); }

 /* Choosing a picture, where there is no screen to take (a phone) */
 #pick { position:fixed; inset:0; display:flex; flex-direction:column; align-items:center; justify-content:center;
   gap:var(--s4); padding:var(--s5); text-align:center; }
 #pick .say { color:var(--dim); max-width:32em; }
 #pick label { display:inline-flex; align-items:center; height:36px; padding:0 var(--s5); border-radius:var(--r-ctl);
   background:var(--accent); color:var(--bg); font-weight:600; cursor:pointer; }
 #pick input { display:none; }

 /* Framing: the picture, still, with everything outside the frame pushed back */
 #stage { position:fixed; inset:0; cursor:crosshair; }
 #shot { position:absolute; left:0; top:0; width:100%; height:100%; display:block; }
 #shade { position:absolute; inset:0; background:#0007; pointer-events:none; }
 #frame { position:absolute; border:1px solid var(--brand); pointer-events:none;
   box-shadow:0 0 0 1px #0009, 0 0 0 100000px #0007; }
 #size { position:absolute; padding:2px var(--s2); border-radius:var(--r-chip); background:var(--brand);
   color:var(--bg); font-size:12px; pointer-events:none; white-space:nowrap; }
 /* Choosing the tool for a frame, beside it */
 #choose { position:fixed; z-index:30; display:flex; flex-direction:column; gap:2px; min-width:200px; padding:var(--s2);
   background:var(--panel); border:1px solid var(--line); border-radius:var(--r-card); box-shadow:0 8px 24px #0007; }
 #choose .say { font-size:12px; color:var(--dim); padding:var(--s1) var(--s2) var(--s2); }
 #choose button { display:flex; align-items:center; justify-content:space-between; gap:var(--s4);
   background:none; border-color:transparent; text-align:left; }
 #choose button:hover, #choose button:focus-visible { background:var(--panel2); border-color:var(--edge); outline:none; }
 kbd { font-family:ui-monospace,Consolas,"Courier New",monospace; font-size:11px; line-height:1.4; padding:0 6px;
   border:1px solid var(--line); border-radius:var(--r-chip); color:var(--dim); background:var(--bg); }
 #hint { position:fixed; left:50%; top:var(--s4); transform:translateX(-50%); padding:var(--s2) var(--s4);
   border-radius:var(--r-card); background:var(--panel); border:1px solid var(--line); font-size:13px;
   pointer-events:none; white-space:nowrap; box-shadow:0 8px 24px #0007; }

 /* Reading a colour: the framed part, grown until one pixel is a square you can hit */
 #zoom { position:fixed; inset:0; display:flex; background:var(--bg); }
 #area { position:relative; flex:1; min-width:0; cursor:none; }
 #zc { position:absolute; inset:0; width:100%; height:100%; display:block; }
 #panel { width:320px; flex:none; display:flex; flex-direction:column; border-left:1px solid var(--line);
   background:var(--panel); min-height:0; }
 #panel h2 { margin:0; font-size:13px; font-weight:600; color:var(--dim); padding:var(--s4) var(--s4) var(--s2); }
 .phead { display:flex; align-items:center; gap:var(--s2); padding:var(--s3) var(--s3) 0 var(--s4); }
 .phead b { flex:1; font-size:14px; }
 button.quiet { background:none; border-color:transparent; color:var(--dim); }
 button.quiet:hover { color:var(--text); border-color:var(--line); }
 .now { display:flex; gap:var(--s3); align-items:center; padding:0 var(--s4) var(--s4); border-bottom:1px solid var(--line); }
 .sw { width:44px; height:44px; flex:none; border-radius:var(--r-ctl); border:1px solid var(--line); }
 .codes { display:flex; flex-direction:column; align-items:flex-start; gap:2px; min-width:0; font-size:13px; }
 .say2 { padding:var(--s2) var(--s4); color:var(--dim); font-size:12px; }
 #list { flex:1; overflow-y:auto; padding:0 var(--s2) var(--s2); }
 .row { display:flex; align-items:center; gap:var(--s2); padding:var(--s1) var(--s2); border-radius:var(--r-ctl); }
 .row .sw { width:24px; height:24px; border-radius:var(--r-chip); }
 .row .c { display:flex; flex-wrap:wrap; gap:2px var(--s2); min-width:0; }
 .code { font-family:ui-monospace,Consolas,"Courier New",monospace; font-size:12.5px; background:none; border:none;
   height:auto; padding:1px var(--s1); color:var(--text); border-radius:var(--r-chip); text-align:left; }
 .code:hover { background:var(--panel2); }
 .foot { display:flex; gap:var(--s2); padding:var(--s3) var(--s4); border-top:1px solid var(--line); }
 .foot button { flex:1; min-width:0; }
 button:disabled { opacity:.5; cursor:default; }
 button:disabled:hover { border-color:var(--line); }
 .pane { display:flex; flex-direction:column; flex:1; min-height:0; }

 /* Reading with the AI: where it goes, then what came back */
 #aipane { padding:var(--s2) var(--s4) var(--s3); gap:var(--s3); overflow-y:auto; }
 .state { color:var(--dim); font-size:13px; }
 .state b { color:var(--text); font-weight:600; }
 .problem { color:var(--text); font-size:13px; }
 #text { flex:1; min-height:8em; width:100%; resize:none; padding:var(--s2) var(--s3);
   border:1px solid var(--line); border-radius:var(--r-ctl); background:var(--bg); color:var(--text);
   font:inherit; font-size:13.5px; line-height:1.6; -webkit-user-select:text; user-select:text; }
 #nouns { display:flex; flex-direction:column; gap:var(--s1); }
 #nouns button { text-align:left; height:auto; min-height:32px; padding:var(--s1) var(--s3); }
 #nouns button:first-child { border-color:var(--accent); font-weight:600; }
 #consent { display:flex; flex-direction:column; gap:var(--s3); padding:var(--s3); border:1px solid var(--line);
   border-radius:var(--r-card); background:var(--panel2); }
 #consent h3 { margin:0; font-size:14px; }
 #consent p { margin:0; font-size:13px; }
 #consent label { display:flex; gap:var(--s2); align-items:flex-start; font-size:13px; cursor:pointer; }
 #consent input { margin:3px 0 0; flex:none; width:16px; height:16px; accent-color:var(--accent); }
 @media (max-width:700px), (max-aspect-ratio:1/1) {
   #zoom { flex-direction:column; }
   #panel { width:auto; height:52%; border-left:none; border-top:1px solid var(--line); }
   #area { cursor:default; }
   /* A phone's panel is short: the colour under the finger on one line, so the
      list of colours taken -- the thing being collected -- keeps the room */
   #panel h2.t-now { display:none; }
   .now { padding-bottom:var(--s3); }
   .now .sw { width:32px; height:32px; }
   .now .codes { flex-direction:row; flex-wrap:wrap; gap:0 var(--s2); }
   .say2.t-howto { padding-top:var(--s1); padding-bottom:0; }
   /* Above the buttons at the foot, not over them */
   body #toast { bottom:72px; }
 }

 /* The way out on a touch screen, where there is no Esc to press. In a corner
    of its own, above the picture and the frame alike */
 #x { position:fixed; top:var(--s3); right:var(--s3); z-index:20; }
 #toast { position:fixed; left:50%; bottom:var(--s5); transform:translateX(-50%); padding:var(--s2) var(--s4);
   border-radius:var(--r-card); background:var(--panel); border:1px solid var(--line); box-shadow:0 8px 24px #0007;
   font-size:13px; pointer-events:none; }
/*__EDIT_CSS__*/
</style></head>
<body>
<div id="wait" hidden><span class="n"></span><span class="say"></span></div>
<div id="pick" hidden>
  <div class="say"></div>
  <label><input type="file" accept="image/*"><span class="btn"></span></label>
</div>
<div id="stage" hidden>
  <canvas id="shot"></canvas>
  <div id="shade"></div>
  <div id="frame" hidden></div>
  <div id="size" hidden></div>
</div>
<div id="hint" hidden></div>
<div id="choose" hidden></div>
<div id="zoom" hidden>
  <div id="area"><canvas id="zc"></canvas></div>
  <aside id="panel">
    <div class="phead"><b class="t-tool"></b><button class="quiet" id="shut"></button></div>
    <div class="pane" id="colorpane" hidden>
      <h2 class="t-now"></h2>
      <div class="now"><div class="sw" id="nowsw"></div><div class="codes" id="nowcodes"></div></div>
      <div class="say2 t-howto"></div>
      <h2 class="t-picked"></h2>
      <div id="list"></div>
    </div>
    <div class="pane" id="aipane" hidden>
      <div class="state" id="aistate"></div>
      <div id="consent" hidden>
        <h3 class="t-c-title"></h3>
        <p id="c-where"></p>
        <p id="c-keep"></p>
        <label><input type="checkbox" id="c-ok"><span class="t-c-ok"></span></label>
        <button class="primary" id="c-send" disabled></button>
      </div>
      <div class="problem" id="problem" hidden></div>
      <button id="again" hidden></button>
      <textarea id="text" hidden spellcheck="false"></textarea>
      <div id="nouns" hidden></div>
    </div>
    <div class="foot">
      <button class="primary" id="toclip"></button>
      <button id="tofile"></button>
    </div>
  </aside>
</div>
<!--__EDIT_HTML__-->
<button id="x" class="quiet" hidden></button>
<div id="toast" hidden></div>
<script>
const T = __DICT__;
const TOOLS = __TOOLS__;
const $ = id => document.getElementById(id);
const Q = new URLSearchParams(location.search);
// The tool asked for, or none: the scissors' own key frames first and the tool
// is chosen after, from the frame
const ASKED = TOOLS.includes(Q.get("tool"));
let TOOL = ASKED ? Q.get("tool") : null;
// The letter that chooses each tool while framing
const TOOL_KEYS = {text: "t", noun: "n", color: "k", edit: "e"};
// The window hands messages to the program that opened it; a page in a phone's
// browser has nobody to hand them to and does the same things itself
const HOST = !!(window.ipc && window.ipc.postMessage);
const tell = o => { if (HOST) window.ipc.postMessage(JSON.stringify(o)); };

let img = null;          // the picture everything is read from
let pixels = null;       // the framed part's colours, read once
let fit = null;          // where the picture sits on screen: {x, y, s} in CSS px
let rect = null;         // the framed part, in the picture's own pixels

// ── Saying something ───────────────────────────────
let toastTimer = 0;
function toast(text) {
  const t = $("toast");
  t.textContent = text;
  t.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => { t.hidden = true; }, 1600);
}

// ── Where the answer goes ──────────────────────────
// One place, so the clipboard and the file mean the same thing from every tool
function copyOut(text) {
  if (HOST) {
    tell({act: "copy", text});
  } else if (navigator.clipboard && window.isSecureContext) {
    navigator.clipboard.writeText(text).catch(() => copyByHand(text));
  } else {
    copyByHand(text);
  }
  toast(T["snip.copied"] || "Copied");
}
// A phone's page is served over plain http, where the browser keeps the
// clipboard API to itself. Selecting text and copying it still works
function copyByHand(text) {
  const box = document.createElement("textarea");
  box.value = text;
  box.style.position = "fixed";
  box.style.opacity = "0";
  document.body.append(box);
  box.select();
  try { document.execCommand("copy"); } catch (e) {}
  box.remove();
}
function saveOut(text, name) {
  if (HOST) {
    tell({act: "save", text, name});
    return;
  }
  const a = document.createElement("a");
  a.href = URL.createObjectURL(new Blob([text], {type: "text/plain"}));
  a.download = name;
  document.body.append(a);
  a.click();
  setTimeout(() => { URL.revokeObjectURL(a.href); a.remove(); }, 1000);
}
function shut() {
  if (HOST) { tell({act: "close"}); return; }
  // Opened as a layer over the board: the board takes it down
  if (window.parent !== window) { window.parent.postMessage({snip: "close"}, "*"); return; }
  history.back();
}
document.addEventListener("keydown", e => {
  // The editor answers Esc itself: a cut being chosen, words being typed or
  // drawing nobody has taken anywhere come before closing
  if (e.key === "Escape" && $("edit").hidden) { e.preventDefault(); shut(); }
});

// ── Waiting, before the picture is taken ───────────
// Told by the window, once a second. Nothing here can be pressed: the card is
// left out of the picture and lets the pointer through, so the person can get
// the screen ready underneath it
window.__snipWait = function (left) {
  const w = $("wait");
  w.hidden = false;
  w.querySelector(".n").textContent = String(left);
  w.querySelector(".say").textContent = T["snip.wait"] || "";
};
// The picture has been taken. Asked for by number, so a picture from an earlier
// press can never be the one that arrives
window.__snipFrame = function (n) {
  $("wait").hidden = true;
  const i = new Image();
  i.onload = () => begin(i);
  i.onerror = () => { toast(T["snip.failed"] || ""); setTimeout(shut, 1600); };
  i.src = "/snip/frame.bmp?n=" + encodeURIComponent(n);
};

// ── A picture chosen, where none can be taken ──────
function offerPick() {
  const p = $("pick");
  p.hidden = false;
  p.querySelector(".say").textContent = T["snip.pick.say"] || "";
  p.querySelector(".btn").textContent = T["snip.pick"] || "";
  p.querySelector("input").onchange = e => {
    const f = e.target.files && e.target.files[0];
    if (!f) return;
    const url = URL.createObjectURL(f);
    const i = new Image();
    i.onload = () => { p.hidden = true; begin(i); };
    i.src = url;
  };
}

// ── Framing ────────────────────────────────────────
function begin(picture) {
  img = picture;
  $("stage").hidden = false;
  const hint = $("hint");
  hint.textContent = framingHint();
  hint.hidden = false;
  drawShot();
  window.addEventListener("resize", drawShot);
}
// What to do now, said above the picture
function framingHint() {
  if (!TOOL) return T["snip.frame.hint_choose"] || "";
  const how = T[TOOL === "edit" ? "snip.frame.hint_edit" : "snip.frame.hint"] || "";
  // A tool chosen by its letter says which, since nothing else on screen does
  return ASKED ? how : (T["snip.tool." + TOOL] || TOOL) + " ・ " + how;
}
// The whole picture, as large as the screen allows and never stretched. On
// the window the picture is the screen itself, so this comes out at exactly one
// picture pixel to one screen pixel and the screen looks as if it stopped
function drawShot() {
  const c = $("shot");
  const dpr = window.devicePixelRatio || 1;
  const vw = innerWidth, vh = innerHeight;
  c.width = Math.round(vw * dpr);
  c.height = Math.round(vh * dpr);
  const s = Math.min(vw / img.naturalWidth, vh / img.naturalHeight);
  const w = img.naturalWidth * s, h = img.naturalHeight * s;
  fit = {x: (vw - w) / 2, y: (vh - h) / 2, s};
  const ctx = c.getContext("2d");
  ctx.imageSmoothingEnabled = s * dpr < 1;
  ctx.fillStyle = "#000";
  ctx.fillRect(0, 0, c.width, c.height);
  ctx.drawImage(img, fit.x * dpr, fit.y * dpr, w * dpr, h * dpr);
}
// A point on screen, as a pixel of the picture
const toPicture = (cx, cy) => ({
  x: Math.max(0, Math.min(img.naturalWidth, (cx - fit.x) / fit.s)),
  y: Math.max(0, Math.min(img.naturalHeight, (cy - fit.y) / fit.s)),
});
(function () {
  const stage = $("stage");
  let from = null;
  const show = (a, b) => {
    const x = Math.min(a.x, b.x), y = Math.min(a.y, b.y);
    const w = Math.abs(a.x - b.x), h = Math.abs(a.y - b.y);
    const f = $("frame");
    f.hidden = false;
    $("shade").hidden = true;
    f.style.left = x + "px"; f.style.top = y + "px";
    f.style.width = w + "px"; f.style.height = h + "px";
    const p = toPicture(x, y), q = toPicture(x + w, y + h);
    const size = $("size");
    size.hidden = false;
    size.textContent = Math.round(q.x - p.x) + " × " + Math.round(q.y - p.y);
    size.style.left = Math.min(x, innerWidth - size.offsetWidth - 4) + "px";
    size.style.top = (y > 28 ? y - 26 : (y + h + 30 < innerHeight ? y + h + 4 : y + 4)) + "px";
  };
  stage.addEventListener("pointerdown", e => {
    if (e.button !== 0 || !img) return;
    stage.setPointerCapture(e.pointerId);
    from = {x: e.clientX, y: e.clientY};
    $("hint").hidden = true;
    // Framing again: the choice offered for the last frame is gone with it
    $("choose").hidden = true;
    pending = null;
  });
  stage.addEventListener("pointermove", e => {
    if (from) show(from, {x: e.clientX, y: e.clientY});
  });
  stage.addEventListener("pointerup", e => {
    if (!from) return;
    const a = toPicture(from.x, from.y), b = toPicture(e.clientX, e.clientY);
    from = null;
    if (!TOOL) {
      pending = {a, b, at: {x: e.clientX, y: e.clientY}};
      offerTools();
      return;
    }
    openTool(a, b);
  });
})();

// A frame waiting for its tool
let pending = null;
function offerTools() {
  const box = $("choose");
  box.textContent = "";
  box.append(Object.assign(document.createElement("div"), {className: "say", textContent: T["snip.choose.say"] || ""}));
  for (const t of TOOLS) {
    const b = document.createElement("button");
    b.append(Object.assign(document.createElement("span"), {textContent: T["snip.tool." + t] || t}),
      Object.assign(document.createElement("kbd"), {textContent: TOOL_KEYS[t].toUpperCase()}));
    b.onclick = () => chooseTool(t);
    box.append(b);
  }
  box.hidden = false;
  // Beside the frame, not over what was framed: to its right, else its left,
  // else below, else above, and on the screen whichever it is
  const f = $("frame");
  const r = f.hidden ? {left: pending.at.x, right: pending.at.x, top: pending.at.y, bottom: pending.at.y}
    : f.getBoundingClientRect();
  const w = box.offsetWidth, h = box.offsetHeight, gap = 8;
  let x, y;
  if (r.right + gap + w <= innerWidth) { x = r.right + gap; y = r.top; }
  else if (r.left - gap - w >= 0) { x = r.left - gap - w; y = r.top; }
  else if (r.bottom + gap + h <= innerHeight) { x = r.left; y = r.bottom + gap; }
  else { x = r.left; y = r.top - gap - h; }
  box.style.left = Math.max(gap, Math.min(innerWidth - w - gap, x)) + "px";
  box.style.top = Math.max(gap, Math.min(innerHeight - h - gap, y)) + "px";
  const first = box.querySelector("button");
  if (first) first.focus();
}
function chooseTool(t) {
  if (!pending) return;
  TOOL = t;
  $("choose").hidden = true;
  const {a, b} = pending;
  pending = null;
  openTool(a, b);
}
// A letter chooses the tool: before framing, for the frame to come; with a
// frame waiting, for that frame
document.addEventListener("keydown", e => {
  if (ASKED || $("stage").hidden || e.ctrlKey || e.altKey || e.metaKey) return;
  const t = TOOLS.find(t => TOOL_KEYS[t] === e.key.toLowerCase());
  if (!t) return;
  e.preventDefault();
  if (pending) return chooseTool(t);
  TOOL = t;
  $("hint").textContent = framingHint();
  $("hint").hidden = false;
});

// The framed part, in the picture's own pixels, and the tool opened over it
function openTool(a, b) {
  let x = Math.floor(Math.min(a.x, b.x)), y = Math.floor(Math.min(a.y, b.y));
  let w = Math.ceil(Math.max(a.x, b.x)) - x, h = Math.ceil(Math.max(a.y, b.y)) - y;
  // A press that did not move frames nothing. Somebody who pressed once
  // meant the place they pressed, so a small square around it stands in
  if ((w < 3 || h < 3) && TOOL === "edit") {
    // A picture to edit is most often the whole of it
    x = 0; y = 0; w = img.naturalWidth; h = img.naturalHeight;
  } else if (w < 3 || h < 3) {
    const R = 12;
    x = Math.max(0, Math.round(a.x) - R); y = Math.max(0, Math.round(a.y) - R);
    w = Math.min(img.naturalWidth - x, R * 2 + 1); h = Math.min(img.naturalHeight - y, R * 2 + 1);
  }
  rect = {x, y, w, h};
  $("stage").hidden = true;
  $("hint").hidden = true;
  if (TOOL === "color") openColor();
  else if (TOOL === "text" || TOOL === "noun") openAi();
  else if (TOOL === "edit") openEdit();
}

// ── Reading a colour ───────────────────────────────
const hex2 = n => n.toString(16).padStart(2, "0");
function codesOf(r, g, b) {
  const R = r / 255, G = g / 255, B = b / 255;
  const max = Math.max(R, G, B), min = Math.min(R, G, B), d = max - min;
  const l = (max + min) / 2;
  let h = 0, s = 0;
  if (d) {
    s = d / (1 - Math.abs(2 * l - 1));
    h = max === R ? ((G - B) / d) % 6 : max === G ? (B - R) / d + 2 : (R - G) / d + 4;
    h = Math.round(h * 60);
    if (h < 0) h += 360;
  }
  return {
    hex: ("#" + hex2(r) + hex2(g) + hex2(b)).toUpperCase(),
    rgb: "rgb(" + r + ", " + g + ", " + b + ")",
    hsl: "hsl(" + h + ", " + Math.round(s * 100) + "%, " + Math.round(l * 100) + "%)",
  };
}
const zoom = {s: 1, ox: 0, oy: 0, cur: null, hover: null};
const picked = [];

function openColor() {
  // The framed part's colours, read once. Reading them from the picture
  // rather than from the screen means the magnified view itself can never be
  // in what is being read
  const off = document.createElement("canvas");
  off.width = rect.w; off.height = rect.h;
  const octx = off.getContext("2d", {willReadFrequently: true});
  octx.drawImage(img, rect.x, rect.y, rect.w, rect.h, 0, 0, rect.w, rect.h);
  pixels = octx.getImageData(0, 0, rect.w, rect.h).data;
  zoom.cur = {x: Math.floor(rect.w / 2), y: Math.floor(rect.h / 2)};
  document.querySelector(".t-tool").textContent = T["snip.tool.color"] || "";
  $("colorpane").hidden = false;
  document.querySelector(".t-now").textContent = T["snip.color.now"] || "";
  // What a finger does and what a mouse and keyboard do are said differently
  const touch = window.matchMedia && matchMedia("(pointer: coarse)").matches;
  document.querySelector(".t-howto").textContent =
    (touch ? T["snip.color.howto_touch"] : T["snip.color.howto"]) || "";
  document.querySelector(".t-picked").textContent = T["snip.color.picked"] || "";
  $("toclip").textContent = T["snip.to.clipboard"] || "";
  $("tofile").textContent = T["snip.to.file"] || "";
  $("shut").textContent = T["snip.close"] || "";
  $("zoom").hidden = false;
  fitZoom();
  drawZoom();
  drawNow(zoom.cur);
  drawPicked();
  window.addEventListener("resize", () => { fitZoom(); drawZoom(); });
}
// The largest whole number of screen pixels one picture pixel can take while
// the framed part still fits. Whole, because a pixel drawn 2.7 wide is a row of
// squares of two different sizes, and the edge between them is where a press
// lands on the wrong colour
function fitZoom() {
  const a = $("area");
  const dpr = window.devicePixelRatio || 1;
  const W = a.clientWidth * dpr, H = a.clientHeight * dpr;
  zoom.s = Math.max(1, Math.floor(Math.min(W / rect.w, H / rect.h)));
  zoom.ox = Math.floor((W - rect.w * zoom.s) / 2);
  zoom.oy = Math.floor((H - rect.h * zoom.s) / 2);
}
function drawZoom() {
  const a = $("area"), c = $("zc");
  const dpr = window.devicePixelRatio || 1;
  c.width = Math.round(a.clientWidth * dpr);
  c.height = Math.round(a.clientHeight * dpr);
  const ctx = c.getContext("2d");
  ctx.imageSmoothingEnabled = false;
  ctx.fillStyle = getComputedStyle(document.body).backgroundColor;
  ctx.fillRect(0, 0, c.width, c.height);
  const s = zoom.s;
  ctx.drawImage(img, rect.x, rect.y, rect.w, rect.h, zoom.ox, zoom.oy, rect.w * s, rect.h * s);
  // The edges between pixels, once a pixel is big enough to be counted
  if (s >= 8) {
    ctx.fillStyle = "rgba(128,128,128,0.35)";
    for (let i = 0; i <= rect.w; i++) {
      const x = zoom.ox + i * s;
      if (x >= 0 && x <= c.width) ctx.fillRect(x, Math.max(0, zoom.oy), 1, rect.h * s);
    }
    for (let j = 0; j <= rect.h; j++) {
      const y = zoom.oy + j * s;
      if (y >= 0 && y <= c.height) ctx.fillRect(Math.max(0, zoom.ox), y, rect.w * s, 1);
    }
  }
  // The pixel that a press would take: dark and light, so it shows on any colour
  const at = zoom.hover || zoom.cur;
  if (at) {
    const x = zoom.ox + at.x * s, y = zoom.oy + at.y * s, w = Math.max(s, 3);
    ctx.lineWidth = 2;
    ctx.strokeStyle = "#000";
    ctx.strokeRect(x - 2, y - 2, w + 4, w + 4);
    ctx.strokeStyle = "#fff";
    ctx.strokeRect(x, y, w, w);
  }
}
function colourAt(p) {
  const i = (p.y * rect.w + p.x) * 4;
  return codesOf(pixels[i], pixels[i + 1], pixels[i + 2]);
}
function pixelUnder(e) {
  const a = $("area").getBoundingClientRect();
  const dpr = window.devicePixelRatio || 1;
  const x = Math.floor(((e.clientX - a.left) * dpr - zoom.ox) / zoom.s);
  const y = Math.floor(((e.clientY - a.top) * dpr - zoom.oy) / zoom.s);
  return (x < 0 || y < 0 || x >= rect.w || y >= rect.h) ? null : {x, y};
}
function codeButton(text) {
  return Object.assign(document.createElement("button"), {
    className: "code", textContent: text, title: T["snip.color.copy_one"] || "",
    onclick: () => copyOut(text),
  });
}
function drawNow(p) {
  const box = $("nowcodes");
  box.textContent = "";
  if (!p) return;
  const k = colourAt(p);
  $("nowsw").style.background = k.hex;
  box.append(codeButton(k.hex), codeButton(k.rgb), codeButton(k.hsl));
}
function pick(p) {
  if (!p) return;
  const k = colourAt(p);
  // The same colour taken twice in a row is one colour
  if (!picked.length || picked[picked.length - 1].hex !== k.hex) picked.push(k);
  drawPicked();
  toast((T["snip.color.took"] || "{hex}").replace("{hex}", k.hex));
}
function drawPicked() {
  const list = $("list");
  list.textContent = "";
  if (!picked.length) {
    const none = document.createElement("div");
    none.className = "say2";
    none.textContent = T["snip.color.none"] || "";
    list.append(none);
  }
  for (const k of picked) {
    const row = document.createElement("div");
    row.className = "row";
    const sw = document.createElement("div");
    sw.className = "sw";
    sw.style.background = k.hex;
    const c = document.createElement("div");
    c.className = "c";
    c.append(codeButton(k.hex), codeButton(k.rgb), codeButton(k.hsl));
    row.append(sw, c);
    list.append(row);
  }
  list.scrollTop = list.scrollHeight;
  $("toclip").disabled = $("tofile").disabled = !picked.length;
}
const listText = () => picked.map(k => k.hex + "\t" + k.rgb + "\t" + k.hsl).join("\n") + "\n";
(function () {
  const a = $("area");
  a.addEventListener("pointermove", e => {
    if (!pixels) return;
    zoom.hover = pixelUnder(e);
    if (zoom.hover) zoom.cur = zoom.hover;
    drawZoom();
    drawNow(zoom.cur);
  });
  a.addEventListener("pointerleave", () => { if (pixels) { zoom.hover = null; drawZoom(); } });
  a.addEventListener("pointerdown", e => {
    if (e.button !== 0 || !pixels) return;
    const p = pixelUnder(e);
    if (p) { zoom.cur = p; pick(p); drawZoom(); drawNow(p); }
  });
  // Closer or further, keeping the pixel under the pointer where it is
  a.addEventListener("wheel", e => {
    e.preventDefault();
    if (!pixels) return;
    const r = a.getBoundingClientRect();
    const dpr = window.devicePixelRatio || 1;
    const mx = (e.clientX - r.left) * dpr, my = (e.clientY - r.top) * dpr;
    const fx = (mx - zoom.ox) / zoom.s, fy = (my - zoom.oy) / zoom.s;
    const s = Math.max(1, Math.min(256, zoom.s + (e.deltaY < 0 ? Math.max(1, Math.round(zoom.s * 0.25)) : -Math.max(1, Math.round(zoom.s * 0.2)))));
    zoom.s = s;
    zoom.ox = Math.round(mx - fx * s);
    zoom.oy = Math.round(my - fy * s);
    zoom.hover = pixelUnder(e);
    drawZoom();
  }, {passive: false});
  document.addEventListener("keydown", e => {
    if ($("zoom").hidden || !pixels || !zoom.cur) return;
    const step = {ArrowLeft: [-1, 0], ArrowRight: [1, 0], ArrowUp: [0, -1], ArrowDown: [0, 1]}[e.key];
    if (step) {
      e.preventDefault();
      zoom.hover = null;
      zoom.cur = {
        x: Math.max(0, Math.min(rect.w - 1, zoom.cur.x + step[0])),
        y: Math.max(0, Math.min(rect.h - 1, zoom.cur.y + step[1])),
      };
      drawZoom();
      drawNow(zoom.cur);
    } else if (e.key === "Enter") {
      e.preventDefault();
      pick(zoom.cur);
    }
  });
  $("shut").onclick = shut;
})();
// The two buttons at the foot hand over whatever the open tool has to give
$("toclip").onclick = () => copyOut(outText());
$("tofile").onclick = () => saveOut(outText(), outName());
const outText = () => TOOL === "color" ? listText() : aiText();
const outName = () => TOOL === "color" ? "colors.txt" : TOOL + ".txt";

// ── Reading with the assistant AI ──────────────────
// The picture stays on this page until the desk has agreed to send it: the
// first question carries no picture, only "may one go, and where to"
let asked = 0;
const waiting = new Map();
function toMachine(msg) {
  return new Promise(done => {
    const id = ++asked;
    msg.id = id;
    waiting.set(id, done);
    if (HOST) {
      tell({act: "ask", msg});
    } else if (window.parent !== window) {
      // A phone: the board this page is laid over holds the connection, and
      // asks on the page's behalf
      window.parent.postMessage({snip: "ask", msg}, location.origin);
    } else {
      waiting.delete(id);
      done({state: "unreachable"});
    }
  });
}
window.__snipAnswer = function (o) {
  const done = o && waiting.get(o.id);
  if (!done) return;
  waiting.delete(o.id);
  done(o);
};
window.addEventListener("message", e => {
  if (e.source === window.parent && e.origin === location.origin && e.data && e.data.snipAnswer) {
    window.__snipAnswer(e.data.snipAnswer);
  }
});

let framedPng = "";      // the framed part, as the AI will be handed it
let answer = null;       // what came back: {text} or {lines}
const fill = (s, v) => String(s || "").replace(/\{(\w+)\}/g, (m, k) => (k in v ? v[k] : m));

function framedPicture() {
  // Past this many pixels on its long side the AI shrinks it anyway, and the
  // page would be sending megabytes to have them thrown away
  const LONG = 4096;
  const k = Math.min(1, LONG / Math.max(rect.w, rect.h));
  const c = document.createElement("canvas");
  c.width = Math.max(1, Math.round(rect.w * k));
  c.height = Math.max(1, Math.round(rect.h * k));
  const ctx = c.getContext("2d");
  ctx.imageSmoothingEnabled = k < 1;
  ctx.drawImage(img, rect.x, rect.y, rect.w, rect.h, 0, 0, c.width, c.height);
  return c.toDataURL("image/png").split(",")[1] || "";
}
// The framed part, as large as the space allows, so it is plain what is about
// to be read
function drawFramed() {
  const a = $("area"), c = $("zc");
  const dpr = window.devicePixelRatio || 1;
  c.width = Math.round(a.clientWidth * dpr);
  c.height = Math.round(a.clientHeight * dpr);
  const ctx = c.getContext("2d");
  ctx.fillStyle = getComputedStyle(document.body).backgroundColor;
  ctx.fillRect(0, 0, c.width, c.height);
  const pad = 16 * dpr;
  const s = Math.min((c.width - pad * 2) / rect.w, (c.height - pad * 2) / rect.h, 4 * dpr);
  const w = rect.w * s, h = rect.h * s;
  // A preview to recognise, not pixels to pick: smooth reads better
  ctx.imageSmoothingEnabled = true;
  ctx.imageSmoothingQuality = "high";
  ctx.drawImage(img, rect.x, rect.y, rect.w, rect.h, (c.width - w) / 2, (c.height - h) / 2, w, h);
}
function aiText() {
  if (!answer) return "";
  if (answer.lines) return answer.lines.join("\n") + "\n";
  return $("text").value;
}
// One thing at a time in the pane: the consent, a problem, or the answer
function aiShow(what) {
  $("consent").hidden = what !== "consent";
  $("problem").hidden = what !== "problem";
  $("again").hidden = what !== "problem";
  $("text").hidden = !(what === "answer" && TOOL === "text");
  $("nouns").hidden = !(what === "answer" && TOOL === "noun");
  // Nothing to hand over until there is an answer, and two buttons for it
  // beside a question about consent would read as a way past the question
  document.querySelector(".foot").hidden = what !== "answer";
}
let clock = 0;
function aiState(html) {
  clearInterval(clock);
  $("aistate").textContent = html;
}
function openAi() {
  document.querySelector(".t-tool").textContent = T["snip.tool." + TOOL] || "";
  $("toclip").textContent = T["snip.to.clipboard"] || "";
  $("tofile").textContent = T["snip.to.file"] || "";
  $("shut").textContent = T["snip.close"] || "";
  document.querySelector(".t-c-title").textContent = T["snip.ai.consent.title"] || "";
  document.querySelector(".t-c-ok").textContent = T["snip.ai.consent.ok"] || "";
  $("c-send").textContent = T["snip.ai.consent.send"] || "";
  $("again").textContent = T["snip.ai.again"] || "";
  $("aipane").hidden = false;
  $("area").style.cursor = "default";
  $("zoom").hidden = false;
  drawFramed();
  window.addEventListener("resize", drawFramed);
  framedPng = framedPicture();
  aiShow("none");
  aiState(T["snip.ai.checking"] || "");
  toMachine({do: "check"}).then(settle);
}
// Where a question stands decides what the pane shows next
function settle(r) {
  if (r.state === "ready") return readIt(r.by);
  if (r.state === "consent") return askConsent(r);
  if (r.state === "answer") return showAnswer(r);
  aiState("");
  const key = {
    unsupported: "snip.ai.unsupported", no_assistant: "snip.ai.no_assistant",
    no_desk: "snip.ai.no_desk", unreachable: "snip.ai.unreachable",
  }[r.state];
  $("problem").textContent = key
    ? fill(T[key], {by: r.by || r.chosen || ""})
    : fill(T["snip.ai.failed"], {by: r.by || "", error: r.error || ""});
  aiShow("problem");
}
function askConsent(r) {
  aiState("");
  $("c-where").textContent = fill(T["snip.ai.consent.where"], {by: r.by});
  $("c-keep").textContent = fill(T["snip.ai.consent.keep"], {desk: r.desk});
  $("c-ok").checked = false;
  $("c-send").disabled = true;
  aiShow("consent");
}
$("c-ok").onchange = () => { $("c-send").disabled = !$("c-ok").checked; };
$("c-send").onclick = () => {
  if (!$("c-ok").checked) return;
  $("c-send").disabled = true;
  aiShow("none");
  aiState(T["snip.ai.checking"] || "");
  toMachine({do: "agree"}).then(settle);
};
$("again").onclick = () => {
  aiShow("none");
  aiState(T["snip.ai.checking"] || "");
  toMachine({do: "check"}).then(settle);
};
// Sent, and said so the whole time: to which AI, and for how long
function readIt(by) {
  aiShow("none");
  const started = Date.now();
  const say = () => {
    $("aistate").textContent = fill(T["snip.ai.reading"], {by, n: Math.floor((Date.now() - started) / 1000)});
  };
  clearInterval(clock);
  say();
  clock = setInterval(say, 1000);
  toMachine({do: "ask", tool: TOOL, png: framedPng}).then(settle);
}
function showAnswer(r) {
  aiState(fill(T["snip.ai.answered"], {by: r.by || ""}));
  answer = r.lines ? {lines: r.lines} : {text: r.text || ""};
  if (r.lines) {
    const box = $("nouns");
    box.textContent = "";
    if (!r.lines.length) box.append(Object.assign(document.createElement("div"),
      {className: "state", textContent: T["snip.ai.nothing"] || ""}));
    for (const w of r.lines) {
      box.append(Object.assign(document.createElement("button"), {
        textContent: w, title: T["snip.ai.copy_one"] || "", onclick: () => copyOut(w),
      }));
    }
  } else {
    $("text").value = answer.text;
  }
  aiShow("answer");
  if (!r.lines && !answer.text) {
    $("problem").textContent = T["snip.ai.nothing"] || "";
    $("problem").hidden = false;
  }
}

// A page on a phone has no Esc key, so it is given a button instead. The
// window says "Esc to cancel" where the frame is drawn, and keeps its screen
// clear of anything that is not the picture
if (!HOST) {
  const x = $("x");
  x.textContent = T["snip.close"] || "";
  x.hidden = false;
  x.onclick = shut;
}
// Each tool has its own way out once it is open
const xObserver = new MutationObserver(() => { if (!HOST) $("x").hidden = !$("zoom").hidden || !$("edit").hidden; });
xObserver.observe($("zoom"), {attributes: true, attributeFilter: ["hidden"]});
xObserver.observe($("edit"), {attributes: true, attributeFilter: ["hidden"]});

// ── Where the picture comes from ───────────────────
// The window says, by number, once it has one; a page with nobody to take a
// picture for it asks the person for one
if (Q.get("src") === "pick" || !HOST) {
  offerPick();
} else if (Q.get("wait")) {
  window.__snipWait(Number(Q.get("wait")));
} else if (Q.get("n")) {
  window.__snipFrame(Q.get("n"));
}
</script>
<!--__EDIT_JS__-->
</body></html>
"##;

#[cfg(test)]
mod tests {
    use super::*;

    /// The page arrives filled in: no marker the builder should have replaced
    /// is left standing, which would turn the whole script into a syntax error
    /// and leave a blank screen over the person's own
    #[test]
    fn the_page_is_filled_in() {
        let page = assembled()
            .replace("{{THEME}}", "")
            .replace("{{SCHEME}}", "dark")
            .replace("{{__lang__}}", "en");
        let built = crate::i18n::render(&page)
            .replace("__DICT__", "{}")
            .replace("__TOOLS__", &serde_json::to_string(TOOLS).unwrap());
        assert!(!built.contains("__DICT__") && !built.contains("__TOOLS__"));
        assert!(built.contains("const TOOLS = [\"text\",\"noun\",\"color\",\"edit\"];"), "the list of tools is not in it");
        assert!(!built.contains("__EDIT_"), "the editor was not laid into the page");
        assert!(built.contains("function openEdit()"), "the editor's script is not in the page");
    }

    /// Every tool the board may offer is one the page can run, and the page
    /// starts a tool only by a name on that list
    #[test]
    fn the_tools_offered_are_the_tools_the_page_runs() {
        for t in TOOLS {
            assert!(
                PAGE.contains(&format!("TOOL === \"{t}\"")),
                "{t} is offered, but the page cannot drive it"
            );
        }
        for t in AI_TOOLS {
            assert!(TOOLS.contains(t), "{t} is not in the list of tools");
            let key = format!("snip.ai.{t}.prompt");
            assert_ne!(crate::i18n::t(&key), key, "{key} is not in the word table");
        }
        assert!(PAGE.contains("let TOOL = ASKED ? Q.get(\"tool\") : null;"), "a tool starts under a name that is not in the list");
        // Every tool can be chosen by a letter, and no two by the same one
        let line = PAGE.lines().find(|l| l.starts_with("const TOOL_KEYS = {")).unwrap();
        let mut letters = Vec::new();
        for t in TOOLS {
            let at = line.find(&format!("{t}: \"")).unwrap_or_else(|| panic!("{t} has no letter to choose it by"));
            letters.push(&line[at + t.len() + 3..at + t.len() + 4]);
        }
        let mut unique = letters.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), letters.len(), "two tools share a letter");
    }

    /// A colour is read in whole pixels. A pixel drawn a fraction wide makes
    /// squares of two sizes, and the edge between them is where a press lands
    /// on the neighbouring colour
    #[test]
    fn a_colour_is_read_in_whole_pixels() {
        assert!(PAGE.contains("zoom.s = Math.max(1, Math.floor(Math.min(W / rect.w, H / rect.h)));"));
        assert!(PAGE.contains("ctx.imageSmoothingEnabled = false;"), "zooming in is blurry");
    }

    /// Nothing the page does with an answer sends it anywhere but the
    /// person's own clipboard or file: the window is told to copy or to save,
    /// and a phone does both itself. The one other thing it tells the window is
    /// a question for [`answer`], which is where sending is decided
    #[test]
    fn an_answer_goes_to_the_clipboard_or_a_file_and_nowhere_else() {
        let page = assembled();
        let acts: Vec<&str> = page.match_indices("tell({act: \"").map(|(i, _)| {
            let rest = &page[i + "tell({act: \"".len()..];
            &rest[..rest.find('"').unwrap()]
        }).collect();
        assert!(!acts.is_empty());
        for a in acts {
            assert!(["copy", "save", "close", "ask", "copy_image", "save_image"].contains(&a), "an unknown destination {a}");
        }
    }

    /// Every tool the board offers has a name to be offered under. The board
    /// builds the key from the tool's name at run time, which the check for
    /// missing words cannot follow, so this one does
    #[test]
    fn every_tool_has_a_name_on_the_menu() {
        for t in TOOLS {
            let key = format!("snip.tool.{t}");
            assert_ne!(crate::i18n::t(&key), key, "{key} is not in the word table");
        }
    }

    /// A picture goes only to the AI the desk agreed to. Agreeing to one AI is
    /// not agreeing to another, and nothing is sent from a desk that is not
    /// there to have agreed
    #[test]
    fn a_picture_goes_only_where_the_desk_agreed() {
        let yes = |_: &str| true;
        let claude = Some(("claude", "Claude Code"));
        assert_eq!(gate(claude, Some(Some("claude")), yes), Gate::Ready { name: "claude", label: "Claude Code" });
        assert_eq!(gate(claude, Some(None), yes), Gate::Consent { name: "claude", label: "Claude Code" });
        assert_eq!(gate(claude, Some(Some("codex")), yes), Gate::Consent { name: "claude", label: "Claude Code" });
        assert_eq!(gate(claude, None, yes), Gate::NoDesk);
        assert_eq!(gate(None, Some(Some("claude")), yes), Gate::NoAssistant);
        assert_eq!(gate(claude, Some(Some("claude")), |_| false), Gate::Unsupported { label: "Claude Code" });
    }

    /// The page's picture is taken only when it is a PNG, and a question with
    /// no desk behind it is refused before anything is read
    #[test]
    fn only_a_png_is_taken_as_the_picture() {
        use base64::Engine as _;
        let b64 = |b: &[u8]| base64::engine::general_purpose::STANDARD.encode(b);
        assert!(picture_of(&b64(b"\x89PNG\r\n\x1a\nrest")).is_some());
        assert!(picture_of(&b64(b"GIF89a")).is_none());
        assert!(picture_of("not base64 !!").is_none());
        assert!(picture_of("").is_none());
        let refused = answer(&serde_json::json!({"do": "ask", "tool": "text", "png": b64(b"\x89PNG\r\n\x1a\n"), "id": 7}), "");
        assert_eq!(refused["state"], "no_desk");
        assert_eq!(refused["id"], 7, "the question's number did not come back");
    }

    /// An answer is taken only in the tool's shape. The object is found
    /// inside whatever the AI put around it; an answer that is not the shape
    /// is no answer, so the AI is asked again rather than shown as said
    #[test]
    fn an_answer_is_taken_only_in_its_shape() {
        let text = |s: &str| read_reply("text", s).map(|v| v["text"].as_str().unwrap_or("?").to_string());
        assert_eq!(text(r#"{"text":"貸借対照表\n2026年"}"#).as_deref(), Some("貸借対照表\n2026年"));
        assert_eq!(text("以下のとおりです。\n```json\n{\"text\": \"A-7731\", \"note\": \"丸は文字ではありません\"}\n```").as_deref(), Some("A-7731"));
        assert_eq!(text("画像に書かれている文字は次のとおりです: 貸借対照表"), None, "it accepted an answer that is not in the shape");
        assert_eq!(text(r#"{"nouns":["猫"]}"#), None, "it accepted another tool's shape");
        assert_eq!(text(r#"{"text": 5}"#), None);

        let nouns = |s: &str| read_reply("noun", s).map(|v| v["lines"].clone());
        assert_eq!(nouns(r#"{"nouns":["1. 猫。","動物","- ペット","・猫"]}"#), Some(serde_json::json!(["猫", "動物", "ペット"])));
        assert_eq!(nouns(r#"{"nouns":["a","b","c","d","e","f"]}"#).unwrap().as_array().unwrap().len(), 5);
        assert_eq!(nouns("猫\n動物"), None);
        assert_eq!(nouns(r#"{"nouns":[1,2]}"#), None);
    }

    /// Every AI tool has a shape to be answered in, and each shape leaves the
    /// AI a place for remarks outside the answer
    #[test]
    fn every_ai_tool_has_a_shape() {
        for t in AI_TOOLS {
            let shape = shape_of(t).unwrap_or_else(|| panic!("{t} has no answer shape"));
            assert!(shape["properties"]["note"].is_object(), "{t} has no place for a note");
            assert_eq!(shape["additionalProperties"], false);
            // Codex CLI refuses a shape that leaves any field optional
            let mut props: Vec<&String> = shape["properties"].as_object().unwrap().keys().collect();
            let mut required: Vec<&str> = shape["required"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
            props.sort();
            required.sort();
            assert_eq!(props, required, "{t}'s shape has a field that is not required");
        }
        assert!(shape_of("color").is_none());
    }

    /// The noun tool names things in the language the app is shown in, and
    /// every translation of its prompt leaves that language as the `{lang}`
    /// blank rather than naming it -- a name is a word a translator turns
    /// into another language's word for the same name
    #[test]
    fn nouns_are_asked_for_in_the_language_on_screen() {
        let dir = crate::repo_root().join("lang");
        let mut seen = 0;
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            let Some(prompt) = v["snip.ai.noun.prompt"].as_str() else { continue };
            assert!(prompt.contains("{lang}"), "the noun prompt in {} has no {{lang}} blank", path.display());
            seen += 1;
        }
        assert!(seen >= 2);
        let filled = prompt_in("noun", "es");
        assert!(filled.contains("es") && !filled.contains("{lang}"), "the language code was not filled in");
    }

    /// Each installed assistant AI, asked about a picture of Japanese text,
    /// names it in the language of the code it was given -- including a
    /// language no translation exists for yet, asked with another's words.
    /// Costs a call per row to each; run by hand with
    /// `SHIKISHA_PICTURE=<png> cargo test -- --ignored nouns_come_back_in`
    #[test]
    #[ignore]
    fn nouns_come_back_in_the_language_asked() {
        let Ok(path) = std::env::var("SHIKISHA_PICTURE") else { return };
        let png = std::fs::read(path).unwrap();
        let shape = shape_of("noun").unwrap().to_string();
        let japanese = |w: &str| w.chars().any(|c| ('\u{3040}'..='\u{30ff}').contains(&c) || ('\u{4e00}'..='\u{9fff}').contains(&c));
        // (the words the prompt is written in, the code it asks for)
        for (words, code) in [("en", "en"), ("ja", "ja"), ("ja", "es")] {
            crate::i18n::init(Some(words), &[crate::repo_root()]);
            let asked = prompt_in("noun", code);
            for name in ["claude", "codex", "gemini"] {
                if crate::webui::assistant_ai(Some(name)).is_none() {
                    continue;
                }
                let said = crate::webui::ask_about_picture(name, &asked, &png, &shape)
                    .unwrap_or_else(|e| panic!("{name} ({words}/{code}) could not read it: {e:#}"));
                let read = read_reply("noun", &said).unwrap_or_else(|| panic!("{name} ({words}/{code}) answered out of shape: {said}"));
                let list: Vec<String> = read["lines"].as_array().unwrap().iter().map(|w| w.as_str().unwrap().to_string()).collect();
                eprintln!("{words}/{code} {name} -> {list:?}");
                match code {
                    "ja" => assert!(list.iter().any(|w| japanese(w)), "{name}, asked for ja, did not answer in Japanese: {list:?}"),
                    _ => assert!(list.iter().all(|w| !japanese(w)), "{name}, asked for {code}, answered in Japanese: {list:?}"),
                }
            }
        }
    }

    /// A part hidden in the editor is hidden for good: blocks or a blur far
    /// larger than a letter, never a thin smear that can be read through
    #[test]
    fn a_hidden_part_is_hidden_for_good() {
        assert!(edit::JS.contains("Math.max(10 * o.unit,"), "the mosaic blocks are too small");
        assert!(edit::JS.contains("Math.max(8 * o.unit,"), "the blur is too weak");
    }

    /// Every word the editor looks up is in the word list. Its keys are built
    /// from tool names at run time, which the check for missing words cannot
    /// follow
    #[test]
    fn the_editor_has_all_its_words() {
        let mut keys: Vec<String> = Vec::new();
        for t in ["arrow", "rect", "text", "hide", "pen"] {
            keys.push(format!("snip.edit.tool.{t}"));
            keys.push(format!("snip.edit.hint.{t}"));
        }
        for k in ["mosaic", "blur", "fill"] { keys.push(format!("snip.edit.how.{k}")); }
        for k in ["thin", "mid", "thick"] { keys.push(format!("snip.edit.width.{k}")); }
        for k in ["small", "mid", "large"] { keys.push(format!("snip.edit.size.{k}")); }
        for k in ["white", "black", "clear"] { keys.push(format!("snip.edit.canvas.bg.{k}")); }
        keys.push("snip.edit.hint.crop".into());
        let src = edit::JS;
        let mut at = 0;
        while let Some(i) = src[at..].find("T[\"") {
            let from = at + i + 3;
            let end = from + src[from..].find('"').unwrap();
            if !src[end + 1..].trim_start().starts_with('+') {
                keys.push(src[from..end].to_string());
            }
            at = end;
        }
        for key in keys {
            assert_ne!(crate::i18n::t(&key), key, "{key} is not in the word list");
        }
    }

    /// The waits offered are short and include none at all -- no wait is how
    /// a picture of this program itself is taken
    #[test]
    fn the_waits_start_at_none() {
        assert_eq!(WAITS.first(), Some(&0));
        assert!(WAITS.iter().all(|w| *w <= 10));
    }
}

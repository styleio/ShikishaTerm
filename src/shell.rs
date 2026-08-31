//! The window's shell. Tab bar, dashboard, status line.
//!
//! This part is written as real HTML. Converting it into character cells
//! straight through would be faster, but that would throw away half the
//! point of having a window at all:
//!   - the ball would stay a text ● forever
//!   - selecting output would drag the tab bar and box-drawing lines along
//!     with it (since everything would be one big grid of cells)
//!   - the duplication with the phone view would never go away
//!
//! Only the terminal's own contents should stay a grid of cells — that part
//! really is a grid, so it's fine.
//!
//! State comes in from uistate. This file never writes "what is happening" —
//! only "how to show it".

/// The shell page. `{{DICT}}` gets the translated strings, `{{BUILD}}` gets the build stamp.
pub const PAGE: &str = r####"<!doctype html><html lang="{{__lang__}}" translate="no"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover">
<!-- Never let a browser machine-translate this page. The terminal is a grid of
     fixed-width cells (see box_of): swapping a run's text for a translation of a
     different length leaves the box the same size, so the rows pile up on top of
     each other. The shell's own wording is already in the user's language, so
     there is nothing here worth translating anyway. The lang attribute above
     stops the mis-detection that sets the offer off in the first place -->
<meta name="google" content="notranslate">
<title>SHIKISHA-TERM</title>
<style>
  :root {
    /* Every colour in one place, written out by the app from the chosen
       scheme: the window's surfaces, the status colours, and the terminal's
       own sixteen as --c0..--c15 */
    {{THEME}}
    /* Which way the browser should draw the parts it draws itself --
       scrollbars, form controls. Getting this wrong leaves white scrollbars
       down the side of a dark window */
    color-scheme:{{SCHEME}};
    /* Prefer fonts that draw box-drawing characters and symbols in one cell.
       Japanese falls back to the monospaced MS Gothic (Meiryo is not monospaced) */
    --mono:{{FONT}};
    /* How big the terminal draws. Changed with Ctrl+wheel and remembered in
       the settings; everything measured from the cell grid follows it */
    --fs:{{FONT_SIZE}}px;
    /* How wide the tab bar is. Dragged by its edge, and 0 when it is put away
       -- one number for both, so "hidden" needs no second piece of state to
       disagree with the first */
    --tabw:{{TAB_W}}px;
  }
  * { box-sizing:border-box; }
  html,body { margin:0; height:100%; overflow:hidden;
    background:var(--bg); color:var(--text); font-family:var(--mono); font-size:14px; }
  /* The terminal itself, and the read-only copies of it in other panes */
  #screen, .pscreen { font-size:var(--fs); }
  #app { position:relative; display:grid; grid-template-columns:auto 1fr;
    grid-template-rows:1fr auto; height:100%; }

  /* ── Left tab bar ───────────────────────── */
  #tabs { grid-row:1/3; width:var(--tabw); background:var(--panel);
    border-right:1px solid var(--line); overflow-y:auto; padding:6px 0;
    display:flex; flex-direction:column; }
  /* Put away, the bar is a width of nothing rather than a display of none: the
     grip below stays exactly where it was, so the way back is where the way
     out was. Its contents must not spill out of a bar that is no longer there */
  #tabs { min-width:0; }
  /* The edge itself. Same handle as the dividers between panes, because it is
     the same gesture and there is no reason to teach it twice. It sits half
     over the boundary and never goes off the left edge, so a bar that has been
     put away can still be caught and pulled back out */
  #tabgrip { position:absolute; top:0; bottom:0; z-index:6; width:9px;
    left:max(0px, calc(var(--tabw) - 4px)); cursor:col-resize; }
  #tabgrip:hover, #tabgrip.dragging { background:var(--brand); opacity:.35; }
  /* Settings lives here as a fixed gear pinned to the very bottom, not a tab */
  .tab.gearrow { margin-top:auto; color:var(--dim); border-top:1px solid var(--line);
    justify-content:center; padding:10px; }
  .tab.gearrow:hover { color:inherit; }
  .tab.gearrow.sel { color:var(--text); }
  .tab.gearrow .gear { font-size:17px; line-height:1; }
  .tab { display:flex; align-items:center; gap:8px; padding:7px 10px;
    cursor:pointer; border-left:3px solid transparent; user-select:none; }
  .tab:hover { background:var(--hover); }
  .tab.sel { background:var(--raise); border-left-color:var(--brand); }
  /* What a tab says it is doing: its own words, under its name. Wraps to a
     second line of its own so it never pushes the row's furniture around */
  .tab { flex-wrap:wrap; }
  .tab .said { flex-basis:100%; margin-left:26px; margin-top:2px; font-size:10px;
    opacity:.6; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
  .tab.sel .said { opacity:.85; }
  /* Where the tab is, above what it last said. Quieter than the name and
     quieter than the news: it is the thing you scan down the column for,
     not the thing you look at */
  /* Laid out so the short precious parts survive a narrow sidebar. The branch
     is the only piece that can be any length, so it is the only one allowed to
     shrink; a pull request number that got clipped would be the one thing on
     the row nobody could have guessed */
  .sub .selfcost { color:var(--dim); font-variant-numeric:tabular-nums; }
  .tab .place { flex-basis:100%; margin-left:26px; margin-top:2px; font-size:10px;
    color:var(--dim); display:flex; gap:6px; align-items:baseline; min-width:0; }
  .tab .place .br { min-width:0; overflow:hidden; text-overflow:ellipsis;
    white-space:nowrap; }
  .tab .place .pr, .tab .place .pt { flex:none; white-space:nowrap; }
  .tab .place .pr { color:var(--brand); }
  .dot { width:8px; height:8px; border-radius:50%; flex:none; background:var(--dim); }
  /* It blinks between two values instead of gliding between them. A glide has
     to be redrawn on every frame the display shows, for as long as an agent is
     at work -- which is measurably a fifth of a processor core spent on one
     8-pixel dot, and it is spent precisely while its owner is trying to type.
     Two values a second read as "alive" just as well and cost two redraws */
  .dot.BUSY, .dot.Working { background:var(--live); animation:pulse 1.2s step-end infinite; }
  .dot.DONE { background:var(--brand); }
  /* The state a person has to answer. Named for the state itself: the class
     is the label the app sends (`QUESTION`), and while this rule said `ASK`
     it matched nothing at all — the one dot that exists to be noticed was
     drawn in the same grey as a tab sitting idle */
  .dot.QUESTION { background:var(--warn); }
  .dot.EXIT { background:var(--stop); }
  @keyframes pulse { 0%,100% { opacity:1 } 50% { opacity:.35 } }
  .num { color:var(--dim); font-size:12px; min-width:14px; }
  .nm { flex:1; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
  .lock { color:var(--warn); font-size:11px; }
  /* The "+" stays muted by default; it only reaches full contrast on hover/touch */
  .tab.addtab { color:var(--dim); }
  .tab.addtab:hover { color:inherit; }
  /* Workspace switcher above INDEX. Clicking it opens the workspace list popup */
  .tab.wsrow { color:var(--dim); font-weight:700; border-bottom:1px solid var(--line); }
  .tab.wsrow:hover { color:inherit; }
  .tab.wsrow .wscaret { margin-left:auto; font-size:11px; }
  /* Make the workspace name in the header/footer clickable */
  .wslink { cursor:pointer; }
  .wslink:hover { color:var(--text); text-decoration:underline; }
  /* Output volume as a real bar chart, not characters */
  .spark { display:flex; align-items:flex-end; gap:1px; height:14px; flex:none; }
  .spark i { width:2px; background:var(--brand); opacity:.75; }

  /* AI tabs, branded per model. --ai is kept separate from --brand so the
     status dot (--brand for DONE) never loses its meaning. Running several
     AIs side by side is the headline feature, so let each one wear its colour.
     The colour is worn twice -- the left bar and the name -- and no more: a
     third copy as a chip in front of the status dot pushed the dot sideways on
     AI rows only, so the column of dots you scan down the sidebar zig-zagged,
     and the second line's fixed indent no longer lined up under anything. */
  .tab.aitab { border-left-color:var(--ai); }
  .tab.aitab.sel { border-left-color:var(--ai); }
  .tab.aitab .nm { color:var(--ai); font-weight:600; }
  .ai-claude   { --ai:#d97757; }
  .ai-codex    { --ai:#19c37d; }
  .ai-gemini   { --ai:#4285f4; }
  .ai-deepseek { --ai:#5b7cff; }
  .ai-qwen     { --ai:#a06bff; }
  .ai-aider    { --ai:#e5644d; }
  .ai-kimi     { --ai:#12b3a8; }

  /* A folder, as a heading over the tabs working in it. Drawn only when there
     is more than one, so nobody meets the idea before they need it.
     The colour says which project, and branches of one project share it --
     it sits in the dot's column so the row below still reads as one line down
     the sidebar, and never on the left edge, which belongs to the AI's own
     colour */
  .tab.folder { padding-top:9px; padding-bottom:3px; gap:6px; }
  .tab.folder .nm { font-size:11px; opacity:.75; letter-spacing:.02em; }
  .tab.folder .chip { width:8px; height:8px; border-radius:2px; flex:0 0 auto;
    background:var(--line); }
  .tab.folder .cut { font-size:11px; opacity:.6; }
  /* A branch of a project sits under the checkout it was cut from, one step
     in. Only the heading moves: the tabs below keep their own column, so the
     status dots still read as one line all the way down the sidebar */
  .tab.folder.cut, .tab.under { padding-left:20px; }
  .tab.folder .hang { color:var(--dim); font-size:11px; margin-left:-10px; opacity:.7; }
  /* Choosing one. The swatches are the colours picked from when nobody has,
     and the last square opens whatever the system offers */
  .swatches { display:flex; flex-wrap:wrap; gap:6px; padding:6px 8px 8px; max-width:200px; }
  .swatches i { width:20px; height:20px; border-radius:5px; cursor:pointer;
    border:1px solid #0004; display:block; }
  .swatches i.any { background:conic-gradient(red,yellow,lime,aqua,blue,magenta,red); }
  .swatches input { position:absolute; width:0; height:0; opacity:0; }

  /* Hamburger and scrim. Not shown on wide screens (sidebar stays visible) */
  #hamburger { display:none; position:fixed; top:6px; left:6px; z-index:40;
    width:34px; height:30px; align-items:center; justify-content:center;
    background:var(--panel); border:1px solid var(--line); border-radius:6px;
    color:var(--text); font-size:16px; cursor:pointer; }
  #backdrop { display:none; }

  /* ── Content area ───────────────────────── */
  /* --fx/--fy/--fw/--fh are the focused pane's rectangle. Undivided they are
     the whole area, which is what every layer below used to hard-code, so a
     single pane renders exactly as it always did. --navh is the browser bar's
     height, reserved out of the top of that rectangle. */
  /* The focused pane as four insets from the content area's edges. Insets
     rather than a position and a size, because most of the chrome below is
     anchored to an edge (the composer to the bottom, the browser bar to the
     top) and would otherwise need its own arithmetic. Undivided they are all
     zero, which is exactly what these rules hard-coded before panes existed. */
  #main { position:relative; overflow:hidden;
    --fx:0px; --fy:0px; --fr:0px; --fb:0px; --navh:0px; }
  /* The panes themselves. Only the ones that aren't focused draw anything here
     — the focused pane's rectangle is filled by the full renderer above. */
  #panes { position:absolute; inset:0; }
  .pane { position:absolute; overflow:hidden; background:var(--bg); }
  .pane.focused { pointer-events:none; }
  .pane .phead { pointer-events:auto; display:none; align-items:center; gap:6px;
    height:22px; padding:0 8px; font-size:11px; cursor:pointer; user-select:none;
    background:var(--panel); border-bottom:1px solid var(--line); color:var(--dim); }
  .pane.focused .phead { color:var(--text); background:var(--raise);
    border-bottom-color:var(--brand); }
  .pane .phead .nm { flex:1; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
  .pane .phead .cl { opacity:.6; padding:0 2px; }
  .pane .phead .cl:hover { opacity:1; color:var(--stop); }
  /* Divide this pane. Next to the ✕ because the pair is the same thought:
     one makes a pane, the other unmakes it */
  .pane .phead .sp { opacity:.55; padding:0 2px; font-size:12px; line-height:1; }
  .pane .phead .sp:hover { opacity:1; color:var(--brand); }
  /* Relaunch what is in this pane. Two of them, because there are two things a
     person means by it -- carry the conversation on, or start it clean -- and
     one control that silently picks for you is how a restart eats a day's work.
     Armed shows in warning colour rather than in words: the caption is 22px
     tall and has no room for "SURE?" */
  .pane .phead .rs { opacity:.55; padding:0 2px; font-size:12px; line-height:1; }
  .pane .phead .rs:hover { opacity:1; color:var(--brand); }
  .pane .phead .rs.armed { opacity:1; color:var(--warn); }
  /* The dividers. Drawn wider than they look so they can actually be grabbed:
     a 1px line is a line, not a handle.
     The handle takes the pointer; the hairline inside it is what the eye sees.
     This used to say the boundary was drawn by the panes either side -- and
     the panes have no border, so between two dark terminals there was nothing
     drawn at all and they read as one. The only place it happened to show was
     beside a placed browser, which is held back from the divider so the whole
     handle stays catchable: a gap by accident, not a line by intent */
  /* A pane with nothing in it. Division is not refused for want of a free
     tab -- the room is made first and filled after, which is the order a
     person does it in */
  .pane .pnew { position:absolute; inset:0; display:none; align-items:center;
    justify-content:center; cursor:pointer; color:var(--dim); font-size:13px;
    /* The focused pane lets the pointer through to whatever is drawn over it,
       and the layers that do that -- the terminal, the board, a placed page --
       are painted above #panes whether or not they have anything to show. An
       empty pane has none of them, and this is the only thing in it worth
       pressing, so it comes up to their level and takes the pointer itself */
    pointer-events:auto; z-index:4; }
  .pane.empty .pnew { display:flex; }
  .pane .pnew:hover { color:var(--brand); background:var(--hover); }
  .pdiv { position:absolute; z-index:3; }
  .pdiv.v { cursor:col-resize; }
  .pdiv.h { cursor:row-resize; }
  .pdiv::after { content:""; position:absolute; background:var(--line); }
  .pdiv.v::after { top:0; bottom:0; left:50%; width:1px; transform:translateX(-50%); }
  .pdiv.h::after { left:0; right:0; top:50%; height:1px; transform:translateY(-50%); }
  /* Under the hand it is a handle, not a rule: the whole width lights up so
     what can be grabbed is what is shown */
  .pdiv:hover::after, .pdiv.dragging::after { opacity:0; }
  .pdiv:hover, .pdiv.dragging { background:var(--brand); opacity:.35; }
  /* While a divider is being dragged the pointer must not be stolen by an
     iframe, a browser layer, or a text selection that starts mid-drag */
  body.dragdiv { user-select:none; }
  body.dragdiv * { pointer-events:none; }
  /* Everything you can take hold of has to be exempted here, not just the pane
     dividers. The tab bar's edge was left out, and because a click begins with
     a mousedown the grip switched itself off between the two halves of a
     double-click -- so the one gesture that puts the width back never arrived */
  body.dragdiv .pdiv, body.dragdiv #tabgrip { pointer-events:auto; }
  .pane .pbody { position:absolute; left:0; right:0; top:0; bottom:0; overflow:hidden; }
  .pane.headed .phead { display:flex; }
  .pane.headed .pbody { top:22px; }
  /* A pane that isn't focused shows its terminal read-only. Same cell grid as
     the real one — it is the same HTML, from the same renderer */
  .pscreen { margin:0; padding:8px; white-space:pre; line-height:1.25;
    font-family:var(--mono); --cw:1ch; }
  /* Declaring font-family here isn't cosmetic. Browsers apply their own
     monospace to <pre>, and that wins over whatever body inherits.
     Skip it, and the chosen font only ever applies to the terminal contents */
  /* --cw is the width of a single cell, measured and set by the page
     (so the content and the cursor are placed using the same number) */
  #screen { position:absolute; left:var(--fx); top:var(--fy); right:var(--fr);
    bottom:var(--fb); margin:0; padding:8px; white-space:pre;
    overflow:auto; line-height:1.25; font-family:var(--mono); --cw:1ch; }
  /* One element per terminal row, so a screen that changed in one place can
     be repaired in one place. A row with nothing on it still has to stand its
     full height, or the rows below would climb up past the cursor */
  #screen .r { min-height:1.25em; }
  /* Screen relay. While viewing a browser tab, this shows in place of the
     terminal. Keeps the aspect ratio while fitting the frame.
     touch-action:none stops the default scroll so finger movement can be
     forwarded as a raw motion trail instead */
  /* The width and the height are spelled out instead of being left to the four
     insets. A canvas is a replaced element, and one whose size is `auto` takes
     the size of what it holds -- here the relayed frame, which is the PC's own
     page and is far wider than any phone. `right` and `bottom` are then
     over-constrained and the browser simply drops them (CSS 2.1 10.3.8), so the
     picture hung off the right edge with no way to reach it. Worse, the phone
     reports its screen shape from this very box: it kept reporting the frame's
     own shape straight back, so the PC never re-shaped the page to the phone
     and the black band under the picture could never close. The insets stay --
     everything else is placed from them -- and these two say the same
     rectangle in the terms a replaced element is willing to hear */
  #cast { position:absolute; left:var(--fx); top:calc(var(--fy) + var(--navh));
    right:var(--fr); bottom:var(--fb);
    width:calc(100% - var(--fx) - var(--fr));
    height:calc(100% - var(--fy) - var(--navh) - var(--fb));
    object-fit:contain; object-position:top center; background:#000; touch-action:none;
    transform-origin:0 0; }
  /* Trackpad-style synthetic cursor: a Windows-like arrow whose tip is the
     click point. The negative margin aligns the arrow tip (SVG coords 2,1)
     exactly with left/top */
  #castcursor { position:absolute; width:19px; height:30px; margin:-2px 0 0 -2px;
    pointer-events:none; z-index:15; display:none;
    filter:drop-shadow(0 1px 2px rgba(0,0,0,.6)); }
  #castcursor svg { display:block; }
  /* Click ripple (feedback that the tap registered) */
  .ripple { position:absolute; width:10px; height:10px; margin:-5px 0 0 -5px;
    border-radius:50%; border:2px solid var(--brand); pointer-events:none; z-index:14;
    animation:rip .48s ease-out forwards; }
  @keyframes rip { from { transform:scale(.4); opacity:.9 } to { transform:scale(4.5); opacity:0 } }
  /* Text input bar (bottom-of-screen input field with an IME preview).
     Its seam with the row of buttons above is a plain rule, not the accent.
     The accent already means one thing on a horizontal line -- "the focus is
     here", which is how the focused pane underlines its caption -- and this
     seam borrowing it put a second one inside the very pane the first had just
     marked out. The eye reads a blue rule as "a pane starts here" and finds a
     boundary that is not there. Full-strength brand is for state; structure
     inside a panel is drawn with --line */
  #castbar { display:flex; align-items:flex-end; gap:6px; padding:6px 8px;
    background:var(--panel); border-top:1px solid var(--line); }
  #castinput { flex:1; min-width:0; font-family:inherit; font-size:16px;
    line-height:1.35; padding:8px 10px; background:var(--bg); color:var(--text);
    border:1px solid var(--line); border-radius:8px; outline:none;
    resize:none; overflow-y:auto; max-height:40vh; }
  /* The hint is one line or it is nothing. The field is the narrowest thing in
     the row (the clip, backspace, send and close buttons around it are fixed), so
     a wrapped placeholder does not grow the field - its second line is simply cut
     in half. Held to one line, the overflow is cut cleanly at the right edge
     instead, whatever a given language's wording came out to */
  #castinput::placeholder { white-space:nowrap; }
  #castbar .castsend { padding:8px 14px; border:0; border-radius:8px;
    background:var(--brand); color:#04121c; font-weight:700; cursor:pointer; }
  #castbar .castbtn { padding:8px 11px; border:1px solid var(--line);
    border-radius:8px; background:var(--bg); color:var(--text); cursor:pointer; }
  /* Indicator shown while in control mode (tap to release) */
  /* Release banner. Placed at the top of the dock so it rides up and down
     together with the auxiliary key row and the keyboard. Avoids both
     failure modes: pinned to the bottom it hides under the keyboard,
     pinned to the top it gets in the way at the top of the screen */
  #castmode { background:var(--raise); border-top:1px solid var(--brand);
    color:var(--text); padding:8px 14px; font-size:13px; text-align:center;
    cursor:pointer; user-select:none; }
  #castmode:active { background:var(--raise); }
  /* Bottom dock combining the auxiliary key row and the text input bar.
     It sits at the foot of the *focused* pane, so the bar you type into and
     the pane you are typing at are the same rectangle. The phone's on-screen
     keyboard lifts it by --kbd on top of that; that lift is a separate term
     rather than a written-over bottom, because a keyboard height assigned
     straight to `bottom` erased the pane offset and pinned the bar to the
     window's floor -- summoned from the top pane, it appeared at the bottom
     one. The key row scrolls horizontally, the input sits on the bottom row */
  #castdock { position:absolute; left:var(--fx); right:var(--fr);
    bottom:calc(var(--fb) + var(--kbd, 0px)); z-index:18;
    display:none; flex-direction:column; }
  /* Desktop-only summon button for the composer bar (bottom-right, above the bar). */
  #composerfab { position:absolute; right:calc(var(--fr) + 16px);
    bottom:calc(var(--fb) + 16px); z-index:19;
    width:44px; height:44px; border-radius:50%; font-size:19px; line-height:1;
    display:flex; align-items:center; justify-content:center; cursor:pointer;
    border:1px solid var(--brand); background:var(--panel); color:var(--brand);
    box-shadow:0 4px 16px rgba(0,0,0,.45); opacity:.85; transition:opacity .15s ease; }
  #composerfab:hover { opacity:1; }
  /* The switchable panel: a fixed switcher (left) + the scrolling content (right). */
  #castpanel { display:flex; align-items:center; gap:8px; padding:0 8px;
    background:var(--panel); border-top:1px solid var(--line); }
  .castswitch { flex:none; margin:6px 0; padding:6px 8px; font-size:13px;
    background:var(--bg); color:var(--text); border:1px solid var(--line);
    border-radius:8px; }
  .castpanelhint { flex:1 1 0; padding:10px 4px; color:var(--dim); font-size:13px; }
  /* The "🎯 still aimed" chip: visible on EVERY panel while a target is set,
     because the composer's Send goes to the operate goal, not the terminal.
     Its ✕ releases the target. */
  .castchip { flex:none; display:inline-flex; align-items:center; gap:6px; margin:6px 0;
    padding:4px 6px 4px 10px; font-size:13px; color:var(--text);
    background:var(--bg); border:1px solid var(--brand); border-radius:999px; }
  .castchipx { flex:none; border:0; background:none; color:var(--dim); font-size:13px;
    cursor:pointer; padding:2px 6px; border-radius:999px; }
  .castchipx:hover { color:var(--text); background:var(--line); }
  /* Fixed ⚙ at the right of the actions panel — edit the quick actions in settings. */
  .castgear { flex:none; margin:6px 0; padding:6px 8px; font-size:14px; cursor:pointer;
    background:var(--bg); color:var(--text); border:1px solid var(--line); border-radius:8px; }
  .castgear:active { background:var(--brand); color:#04121c; }
  /* Keys / actions rows fill the rest and scroll horizontally under the switcher. */
  #castkeys, #castactions { display:flex; gap:6px; overflow-x:auto; white-space:nowrap;
    flex:1 1 0; min-width:0; padding:6px 0;
    -webkit-overflow-scrolling:touch; scrollbar-width:none; }
  #castkeys::-webkit-scrollbar, #castactions::-webkit-scrollbar { display:none; }
  #casttarget { display:flex; align-items:center; gap:8px; flex:1 1 0; min-width:0;
    padding:6px 0; overflow-x:auto; white-space:nowrap; scrollbar-width:none; }
  #casttarget::-webkit-scrollbar { display:none; }
  /* 📼 record/run: two radios + a hint, same row shape as 🎯. */
  #castlua { display:flex; align-items:center; gap:12px; flex:1 1 0; min-width:0;
    padding:6px 0; overflow-x:auto; white-space:nowrap; scrollbar-width:none; }
  #castlua::-webkit-scrollbar { display:none; }
  .castradio { flex:none; display:flex; align-items:center; gap:5px; font-size:13px;
    color:var(--text); cursor:pointer; user-select:none; }
  .castradio input { accent-color:var(--brand); margin:0; }
  .castaction { flex:0 0 auto; max-width:60vw; overflow:hidden; text-overflow:ellipsis;
    padding:7px 12px; font-size:13px; border:1px solid var(--brand); border-radius:14px;
    background:color-mix(in srgb, var(--brand) 10%, transparent); color:var(--text); cursor:pointer; user-select:none; }
  .castaction:active { background:var(--brand); color:#04121c; }
  /* Lua actions run on tap (rather than filling the composer) — mark them. */
  .castaction.lua { border-style:dashed; }
  .castaction.lua::before { content:"▶"; margin-right:4px; opacity:.85; }
  .castkey { flex:0 0 auto; min-width:40px; padding:8px 10px; font-size:14px;
    border:1px solid var(--line); border-radius:8px; background:var(--bg);
    color:var(--text); cursor:pointer; user-select:none; }
  .castkey:active { background:var(--brand); color:#04121c; }
  /* Ctrl/Alt are latching toggles: while held on, light them up and wait for the next keypress */
  .castkey.mod.on { background:var(--brand); color:#04121c; border-color:var(--brand); }
  /* Only this is selectable — the tab bar and frame never get pulled into a selection */
  #screen { user-select:text; }

  /* ── Bar above the browser view ──────────────────
     Never drawn inside the page — the page is pushed down a notch and this
     is drawn in the space that opens up. Drawing inside the page would
     fight with the site's own CSS, disappear on every navigation, and
     cover the site's own fixed header from above */
  #nav { position:absolute; left:var(--fx); top:var(--fy); right:var(--fr); height:36px; z-index:5;
    display:flex; align-items:center; gap:6px; padding:0 8px;
    border-bottom:1px solid var(--line); background:var(--panel);
    transition:background .15s, border-color .15s; }
  #nav[hidden] { display:none; }
  #nav button { font:inherit; font-size:13px; color:var(--text); cursor:pointer;
    background:transparent; border:1px solid var(--line); border-radius:6px;
    width:28px; height:24px; line-height:1; padding:0; flex:none; }
  #nav button:hover:not(:disabled) { background:var(--raise); border-color:var(--brand); }
  #nav button:disabled { color:var(--line); cursor:default; }
  #nav input { flex:1; min-width:60px; font:inherit; font-size:12px;
    color:var(--text); background:var(--bg); border:1px solid var(--line);
    border-radius:6px; padding:3px 8px; outline:none; }
  #nav input:focus { border-color:var(--brand); }
  /* While loading, tint the whole bar blue so it's obvious at a glance
     that something is in flight. Kept lit for at least 0.5s on the app
     side so even a near-instant load is visible */
  #nav.loading { background:var(--tint); border-bottom-color:var(--brand); }
  /* A band of light sweeping along the bottom edge as an added motion cue */
  #nav.loading::after { content:""; position:absolute; left:0; right:0; bottom:0;
    height:3px; background:linear-gradient(90deg,transparent,var(--brand),transparent);
    background-size:40% 100%; background-repeat:no-repeat;
    animation:navload 1s linear infinite; }
  @keyframes navload { from { background-position:-40% 0 } to { background-position:140% 0 } }
  /* The reload button glows blue and spins so it's obvious where to look */
  #nav button.spin { color:var(--brand); border-color:var(--brand); background:var(--tint); }
  #nav button.spin .ico { display:inline-block; animation:spin .8s linear infinite; }
  @keyframes spin { to { transform:rotate(360deg) } }
  /* Where the page sits. Pushed down by exactly the height of the bar above it */
  /* Where a browser placed in the focused pane sits. The room held back for
     the composer (or for the pen that summons it) is --dock, added to the
     pane's own offset rather than written over it: a height assigned straight
     to `bottom` threw the pane away, and the browser then painted down the
     whole column -- over the pane below it, and over the pen it was supposed
     to be making room for. Same lesson as #castdock above */
  #page { position:absolute; left:var(--fx); top:calc(var(--fy) + var(--navh));
    right:var(--fr); bottom:calc(var(--fb) + var(--dock, 0px)); pointer-events:none; }

  /* ── Discussion topic banner ─────────────────────
     A prominent prompt floated over whatever tab is in view while an AI-vs-AI
     discussion is at rest. Type a topic → it's sent to the opening speaker and
     the round begins. It hides itself the moment a participant starts speaking,
     so the AI screens are never covered, and returns when the round finishes so
     the next topic can be posed. Placed at the very top: the AI CLIs keep their
     input line at the bottom, so this never sits on top of it. */
  #topicbar { position:absolute; left:var(--fx); right:var(--fr); top:var(--fy); z-index:24;
    display:flex; align-items:center; gap:10px; flex-wrap:wrap;
    padding:11px 16px; background:linear-gradient(180deg,var(--tint),var(--panel));
    border-bottom:2px solid var(--live); box-shadow:0 8px 22px rgba(0,0,0,.55); }
  #topicbar[hidden] { display:none; }
  #topicbar .tb-ico { font-size:16px; flex:none; }
  #topicbar .tb-label { font-weight:700; color:var(--text); font-size:13px; flex:none; }
  #topicbar input { flex:1 1 220px; min-width:140px; padding:9px 12px;
    border-radius:8px; border:1px solid var(--line); background:var(--bg);
    color:var(--text); font-size:14px; outline:none; }
  #topicbar input:focus { border-color:var(--live); }
  #topicbar button { flex:none; padding:9px 22px; border-radius:8px; border:0;
    background:var(--live); color:#04121c; font-weight:700; cursor:pointer;
    font-size:14px; animation:tbpulse 1.7s ease-in-out 6; }
  #topicbar button:hover { filter:brightness(1.08); }
  /* A soft green ring that breathes outward, to draw the eye without motion
     sickness. It breathes six times and then stops: this bar waits for a
     person, so "forever" means until they get back -- and a shadow that grows
     and blurs is redrawn whole on every frame of it. Catching the eye is what
     the ring is for, and it has done that within ten seconds */
  @keyframes tbpulse { 0%,100% { box-shadow:0 0 0 0 color-mix(in srgb, var(--live) 55%, transparent) }
    50% { box-shadow:0 0 0 7px color-mix(in srgb, var(--live) 0%, transparent) } }
  /* The topic hint drops onto its own line below on narrow widths */
  #topicbar .tb-hint { color:var(--dim); font-size:12px; flex:1 1 100%; margin:-2px 0 0; }
  @media (prefers-reduced-motion: reduce) { #topicbar button { animation:none; } }
  /* Claude-style "thinking" bubble floated over the conversation while a reply
     generates — bouncing dots + a bubble that breathes, so the wait feels alive.
     Sits just above the chat input, bottom-left like a chat app. */
  /* No bubble chrome — the dots + text sit inline right where the cursor is
     (where "generating" used to print), so it reads as part of the
     conversation. left/top are set from the cursor position by JS. */
  #thinking { position:absolute; z-index:22; white-space:nowrap;
    display:inline-flex; align-items:center; gap:8px;
    color:var(--text); font-size:14px; font-weight:500; }
  #thinking[hidden] { display:none; }
  #thinking .th-dots { display:flex; align-items:center; gap:4px; height:16px; }
  #thinking .th-dots span { width:6px; height:6px; background:var(--live);
    border-radius:50%; animation:thinking-dot 1.15s ease-in-out infinite; }
  #thinking .th-dots span:nth-child(2) { animation-delay:.14s; }
  #thinking .th-dots span:nth-child(3) { animation-delay:.28s; }
  #thinking .th-text { animation:text-pulse 2s ease-in-out infinite; }
  /* Elapsed time, so you can see it's still working and not stuck */
  #thinking .th-elapsed { color:var(--dim); font-size:12px; font-weight:400;
    font-variant-numeric:tabular-nums; }
  @keyframes thinking-dot { 0%,60%,100% { transform:translateY(0) scale(.9); opacity:.32 }
    30% { transform:translateY(-5px) scale(1); opacity:1 } }
  @keyframes text-pulse { 0%,100% { opacity:.62 } 50% { opacity:1 } }
  @media (prefers-reduced-motion: reduce) {
    #thinking .th-dots span, #thinking .th-text { animation:none } }

  /* ── Dashboard ─────────────────────────────── */
  /* INDEX is a screen, not a pane: it fills the content area whatever the
     panes underneath are doing. It was drawn into the focused pane's rectangle
     once, which is how it came to be one of them */
  #board { position:absolute; inset:0; overflow:auto; padding:22px 26px; }
  .mark { color:var(--brand); font-weight:700; letter-spacing:.5px;
    font-size:13px; line-height:1.15; white-space:pre; }
  /* Plain title for narrow screens (hidden by default) */
  .mark-lite { display:none; color:var(--brand); font-weight:700;
    font-size:22px; letter-spacing:2px; }
  .sub { color:var(--dim); font-size:12px; margin-top:4px; }
  .card { margin-top:20px; border:1px solid var(--line); border-radius:10px;
    background:var(--panel); padding:14px 16px; }
  .card h2 { margin:0 0 10px; font-size:12px; font-weight:600; color:var(--dim);
    letter-spacing:1px; text-transform:uppercase; }
  /* Chain gauge — a real bar, not a run of ━ characters */
  .gauge { height:8px; border-radius:4px; background:var(--sunk); overflow:hidden; }
  .gauge i { display:block; height:100%; background:var(--live);
    transition:width .3s ease, background .3s ease; }
  .rows { width:100%; border-collapse:collapse; font-size:13px; }
  .rows th { text-align:left; color:var(--dim); font-weight:600; font-size:11px;
    letter-spacing:1px; padding:0 8px 6px; }
  .rows td { padding:5px 8px; border-top:1px solid var(--line); }
  .rows tr { cursor:pointer; }
  .rows tr:hover td { background:var(--hover); }
  /* The cost column is numbers you scan down, not prose: line them up and keep
     them quiet until one is worth noticing */
  .rows td.cost { font-variant-numeric:tabular-nums; color:var(--dim); white-space:nowrap; }
  .menu { display:grid; grid-template-columns:repeat(auto-fill,minmax(230px,1fr));
    gap:6px; }
  .mi { display:flex; gap:9px; align-items:center; padding:7px 9px; border-radius:7px;
    cursor:pointer; }
  .mi:hover { background:var(--hover); }
  /* Only the window can carry this one out. Shown, but plainly not tappable
     from here — a silent no-op would just look broken. */
  .mi.windowonly { cursor:default; opacity:.45; }
  .mi.windowonly:hover { background:none; }
  .mi .only { font-size:11px; color:var(--dim); }
  .row.dim { color:var(--dim); margin-top:8px; }
  .key { font-size:11px; color:#04121c; background:var(--brand); border-radius:4px;
    padding:1px 6px; font-weight:700; }

  /* ── The ball. A real circle that actually moves ────────── */
  #lanes { position:relative; height:44px; margin-top:6px; }
  #ball { position:absolute; width:14px; height:14px; border-radius:50%;
    background:var(--live); box-shadow:0 0 12px var(--live);
    transition:left .35s cubic-bezier(.4,1.4,.5,1), top .35s ease, background .3s;
    transform:translate(-50%,-50%); }
  #ball.human { background:var(--brand); box-shadow:0 0 12px var(--brand); }
  #ball.wait { animation:pulse 1s step-end infinite; }
  .lane { position:absolute; top:50%; height:2px; background:var(--line);
    transform:translateY(-50%); }
  .peg { position:absolute; top:50%; width:7px; height:7px; border-radius:50%;
    background:var(--line); transform:translate(-50%,-50%); }
  .peg b { position:absolute; top:12px; left:50%; transform:translateX(-50%);
    font-size:10px; color:var(--dim); font-weight:400; white-space:nowrap; }

  /* ── Status line ─────────────────────────── */
  #status { grid-column:2; display:flex; align-items:center; gap:12px;
    padding:5px 12px; border-top:1px solid var(--line); background:var(--panel);
    font-size:12px; color:var(--dim); flex-wrap:nowrap; }
  #status .grow { flex:1; }
  /* Only the workspace name gets truncated when space is tight — pills and STOP never shrink */
  #status > span:first-child { min-width:0; white-space:nowrap;
    overflow:hidden; text-overflow:ellipsis; }
  #status .pill, #status .build, #stop, #restart { flex:none; white-space:nowrap; }
  .pill { padding:1px 8px; border-radius:9px; border:1px solid var(--line); }
  .pill.on { color:var(--live); border-color:color-mix(in srgb, var(--live) 40%, var(--bg)); }
  .pill.off { color:var(--dim); }
  .pill.live { color:var(--brand); border-color:var(--brand); cursor:pointer; }
  .pill.live:hover { background:var(--tint); }
  #stop { cursor:pointer; color:var(--stop); border:1px solid color-mix(in srgb, var(--stop) 40%, var(--bg));
    padding:2px 10px; border-radius:7px; font-weight:700; }
  #stop:hover { background:var(--stop); color:var(--bg); }
  /* Relaunch the tab in view. Same shape as the stop button but a notch quieter:
     they sit side by side, and the red one has to stay the one that catches the eye */
  #restart { cursor:pointer; color:var(--dim); border:1px solid var(--line);
    padding:2px 10px; border-radius:7px; font-weight:700; }
  #restart:hover { background:var(--line); color:var(--text); }
  /* Armed - the next press kills and relaunches what is running */
  #restart.armed { color:var(--warn); border-color:color-mix(in srgb, var(--warn) 45%, var(--bg)); background:color-mix(in srgb, var(--warn) 14%, var(--bg)); }
  /* Where input is captured, layered over the cursor.
     The IME candidate window follows this element, so its position becomes
     the candidate window's position too. Text mid-conversion is drawn
     underlined by the browser itself, so this element doesn't draw it */
  #kbd { position:absolute; border:0; padding:0; margin:0; outline:none;
    background:transparent; color:var(--text); caret-color:transparent;
    overflow:hidden; resize:none; white-space:pre; font:inherit;
    line-height:inherit; width:1px; }
  #cur { position:absolute; background:var(--cursor); opacity:.75;
    pointer-events:none; animation:blink 1.06s step-end infinite; }
  @keyframes blink { 0%,50% { opacity:.75 } 50.01%,100% { opacity:0 } }
  ::selection { background:var(--sel); }
  /* The measuring probe needs the same font declaration for the same reason
     — measuring with one thing and drawing with another defeats the point */
  #probe, #tprobe { position:absolute; visibility:hidden; white-space:pre;
    left:0; top:0; margin:0; font-family:var(--mono); }
  /* Overlay screen. Darken outside it so the clickable area stays obvious */
  .dot.WEB { background:var(--brand); }
  #veil { position:fixed; inset:0; background:#00000099; display:flex;
    align-items:center; justify-content:center; z-index:50; }
  /* hidden defaults to display:none, but declaring display yourself wins
     over that default. Since we declared it, we also own hiding it */
  #veil[hidden] { display:none; }
  /* Startup splash. Visible on load, hidden once the first board state arrives.
     Sits below the password veil (z-index 50) so the prompt shows on top of it. */
  #splash { position:fixed; inset:0; z-index:40; display:flex; flex-direction:column;
    align-items:center; justify-content:center; gap:20px; background:var(--bg); }
  #splash[hidden] { display:none; }
  #splash .logo { font-size:26px; letter-spacing:3px; font-weight:700; color:var(--brand); }
  #splash .spin { width:34px; height:34px; border:3px solid var(--line);
    border-top-color:var(--brand); border-radius:50%; animation:splashspin .8s linear infinite; }
  #splash .msg { font-size:13px; color:var(--dim); }
  @keyframes splashspin { to { transform:rotate(360deg); } }

  /* Phone-only overlay shown when the feed stops — a revoked token (a deliberate
     disconnect from the PC) or a dropped link. Above everything so a stale screen
     can't be mistaken for a live one. */
  #netveil { position:fixed; inset:0; z-index:60; display:flex; align-items:center;
    justify-content:center; padding:28px; text-align:center;
    background:color-mix(in srgb, var(--bg) 93%, transparent); -webkit-backdrop-filter:blur(3px); backdrop-filter:blur(3px); }
  #netveil[hidden] { display:none; }
  #netveil .nvbox { max-width:340px; }
  #netveil .nvicon { font-size:46px; line-height:1; margin-bottom:14px; }
  #netveil .nvtitle { font-size:17px; font-weight:700; color:var(--text); margin-bottom:10px; }
  #netveil .nvsub { font-size:13px; color:var(--dim); line-height:1.55; }
  #netveil.cut .nvtitle { color:var(--warn); }
  #netveil .nvbtn { margin-top:18px; padding:11px 22px; border-radius:10px; font-size:14px;
    border:1px solid var(--line); background:var(--panel); color:var(--text); }
  #netveil .nvbtn[hidden] { display:none; }

  #vault, #palette, #branch, #browse { position:fixed; inset:0; background:#00000099; display:flex;
    align-items:flex-start; justify-content:center; z-index:52; padding:8vh 16px 16px; }
  #vault[hidden], #palette[hidden], #branch[hidden], #browse[hidden] { display:none; }
  #vault .vbox, #palette .vbox, #branch .vbox, #browse .vbox { background:var(--panel); border:1px solid var(--brand);
    border-radius:12px; padding:16px 18px; width:min(720px,92vw); max-height:82vh;
    display:flex; flex-direction:column; gap:10px; }
  #vault .vhead, #palette .vhead, #branch .vhead, #browse .vhead { display:flex; align-items:center; }
  #vault .vtitle, #palette .vtitle, #branch .vtitle, #browse .vtitle { color:var(--brand);
    font-size:13px; letter-spacing:1px; text-transform:uppercase; flex:1; }
  #vault .vclose, #palette .vclose, #branch .vclose, #browse .vclose { cursor:pointer;
    color:var(--dim); font-size:16px; padding:2px 6px; }
  #vault .vclose:hover, #palette .vclose:hover, #branch .vclose:hover,
  #browse .vclose:hover { color:var(--text); }
  #vault #vq, #palette #pq, #branch #bq { font:inherit; font-size:14px; background:var(--bg);
    color:var(--text); border:1px solid var(--line); border-radius:8px; padding:9px 12px; outline:none; }
  #vault #vq:focus, #palette #pq:focus, #branch #bq:focus { border-color:var(--brand); }
  /* What is about to happen, said before it does: where the folder will be, and
     the command itself. Never typed into -- the branch name above is the only
     thing anyone fills in */
  #branch .bsay { color:var(--dim); font-size:11.5px; }
  /* The name to give it, and what it starts from. One is typed and the other
     is picked, because one of them is new and the other already exists */
  #branch .brow2 { display:flex; gap:8px; align-items:stretch; }
  #branch .brow2 #bq { flex:1; min-width:0; }
  #branch #bbase { font:inherit; font-size:12.5px; background:var(--bg); color:var(--text);
    border:1px solid var(--line); border-radius:8px; padding:0 8px; outline:none;
    max-width:42%; }
  #browse .vlist { overflow:auto; display:flex; flex-direction:column; gap:2px; max-height:52vh; }
  #browse .vrow { padding:8px 10px; border-radius:8px; cursor:pointer; }
  #browse .vrow:hover { background:var(--raise); }
  #branch .bwhere, #branch .bcmd, #browse .bwhere { font-family:var(--mono); font-size:11.5px; color:var(--text);
    background:var(--bg); border:1px solid var(--line); border-radius:8px; padding:7px 9px;
    overflow:auto; white-space:pre-wrap; word-break:break-all; }
  #branch .bcmd { color:var(--dim); }
  /* What the new folder cannot get from git. Ticked as it will happen, so
     nobody has to read it unless they disagree */
  #branch .bcarry { display:flex; flex-wrap:wrap; gap:6px 14px; align-items:center; }
  #branch .bcarry .say { color:var(--dim); font-size:11.5px; }
  #branch .bcarry label { display:flex; align-items:center; gap:5px; font-size:12px;
    color:var(--text); cursor:pointer; }
  #branch .berr, #browse .berr { color:var(--bad, #e5644d); font-size:12px; white-space:pre-wrap; }
  #branch .brow, #browse .brow { display:flex; gap:8px; justify-content:flex-end; }
  #branch button, #browse button { font:inherit; font-size:13px; padding:7px 16px;
    border-radius:8px; border:1px solid var(--line); background:var(--raise);
    color:var(--text); cursor:pointer; }
  #branch button.go, #browse button.go { border-color:var(--brand); color:var(--brand); }
  #branch button[disabled], #browse button[disabled] { opacity:.45; cursor:default; }
  /* The little menu a folder's heading opens. Not a hover thing: it has to be
     reachable by a finger as well as a pointer */
  .fmenu { position:fixed; z-index:60; background:var(--panel); border:1px solid var(--line);
    border-radius:10px; padding:5px; min-width:190px; box-shadow:0 8px 24px #0007; }
  .fmenu div { padding:8px 10px; border-radius:7px; cursor:pointer; font-size:12.5px; color:var(--text); }
  .fmenu div:hover { background:var(--raise); }
  .fmenu div.warn:hover { color:var(--bad, #e5644d); }
  .fmenu .fname { font:inherit; font-size:12.5px; width:100%; box-sizing:border-box;
    background:var(--bg); color:var(--text); border:1px solid var(--brand);
    border-radius:6px; padding:4px 6px; outline:none; }
  .tab.folder .more { margin-left:auto; padding:0 4px; color:var(--dim); cursor:pointer;
    font-size:13px; line-height:1; }
  .tab.folder .more:hover { color:var(--text); }
  .tab.folder .caret { color:var(--dim); font-size:9px; }
  #vault .vhint { color:var(--dim); font-size:11.5px; }
  #vault .vlist, #palette .vlist { overflow:auto; display:flex; flex-direction:column; gap:2px; }
  #vault .vrow, #palette .prow { padding:9px 10px; border-radius:8px; cursor:pointer; border:1px solid transparent; }
  #palette .prow { display:flex; gap:10px; align-items:baseline; }
  #palette .prow.sel { background:var(--raise); border-color:var(--brand); }
  #palette .pgrp { flex:none; font-size:10px; color:var(--brand); text-transform:uppercase;
    width:64px; letter-spacing:.5px; }
  #palette .plabel { color:var(--text); font-size:13px; overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
  #vault .vrow:hover { background:var(--raise); border-color:var(--line); }
  #vault .vrow .vr1 { display:flex; gap:8px; align-items:baseline; }
  #vault .vrow .vprog { color:var(--brand); font-size:11px; flex:none; }
  #vault .vrow .vname { color:var(--text); font-size:13px; overflow:hidden;
    text-overflow:ellipsis; white-space:nowrap; }
  #vault .vrow .vwhen { color:var(--dim); font-size:11px; margin-left:auto; flex:none; }
  #vault .vrow .vsnip { color:var(--dim); font-size:11.5px; margin-top:2px;
    overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
  #veil .box { background:var(--panel); border:1px solid var(--brand);
    border-radius:12px; padding:20px 24px; max-width:min(760px,86vw);
    max-height:84vh; overflow:auto; }
  #veil h3 { margin:0 0 12px; font-size:13px; color:var(--brand);
    letter-spacing:1px; text-transform:uppercase; }
  #veil .row { display:flex; gap:10px; align-items:center; padding:5px 0;
    font-size:13px; }
  #veil .pick { cursor:pointer; padding:7px 10px; border-radius:7px; }
  #veil .pick:hover { background:var(--raise); }
  #veil .qr { background:#fff; padding:12px; border-radius:8px; }
  #veil .url { font-size:12px; color:var(--dim); margin-top:10px;
    word-break:break-all; user-select:text; }
  /* Marker shown while scrolled back through history. Without it, the output looks like it has frozen */
  #back { position:absolute; right:calc(var(--fr) + 14px); top:calc(var(--fy) + 10px); z-index:6;
    background:var(--raise); border:1px solid var(--brand); color:var(--text);
    padding:4px 12px; border-radius:14px; font-size:12px; cursor:pointer; }
  #back:hover { background:var(--brand); color:#04121c; }
  /* Every message this window shows — the app's own line and the ones the page
     raises itself — is the shared toast (src/toast.rs). Seated inside #main
     rather than the viewport, because #main is the part of the window this
     screen owns, and above the sub-input bar (z-index 18) rather than behind
     it: without a z-index of its own it stacked under the dock, so every
     message shown while the composer was open said nothing to anybody. How
     high it rides is decided per message by toastBottom(), since the composer
     bar is only sometimes there. */
  /* Seated over the FOCUSED pane, not the window. A message is about what is
     in front, and centred on the window it was cut in half by a page placed in
     the other half -- a page is a window of its own and cannot be drawn over.
     (When the focused pane holds such a page, the page draws the message
     itself; see caps::browser_toast.) */
  #toast { --toast-pos:absolute; --toast-z:32;
    --toast-bottom:calc(var(--fb) + 52px);
    --toast-x:calc(var(--fx) + (100% - var(--fx) - var(--fr)) / 2);
    --toast-max:min(calc(100% - var(--fx) - var(--fr) - 24px), 560px); }
{{TOAST_CSS}}

  /* ── Remote history paging (phone only) ──────────────────────────────
     A phone can't scroll a full-screen TUI smoothly over the network, so
     instead of continuous swipe it pages one screenful at a time with two
     buttons — the frame just updates in place (no slide). Hidden by default;
     shown only on a terminal tab, remote. */
  #pageui { position:absolute; right:calc(var(--fr) + 12px);
    top:calc(var(--fy) + (100% - var(--fy) - var(--fb)) / 2); transform:translateY(-50%);
    display:none; flex-direction:column; align-items:center; gap:10px; z-index:8; }
  #pageui.on { display:flex; }
  .pagebtn { width:50px; height:50px; border-radius:50%; border:1px solid var(--line);
    background:color-mix(in srgb, var(--panel) 66%, transparent); color:var(--text); font-size:19px; line-height:1;
    -webkit-backdrop-filter:blur(4px); backdrop-filter:blur(4px);
    display:flex; align-items:center; justify-content:center; cursor:pointer;
    user-select:none; -webkit-user-select:none; touch-action:manipulation; }
  .pagebtn:active { background:color-mix(in srgb, var(--line) 92%, transparent); }
  #pageCount { min-height:16px; font-size:12px; font-weight:700; color:var(--brand);
    text-shadow:0 0 6px rgba(0,0,0,.6);
    display:flex; align-items:center; justify-content:center; }
  /* Shown from the moment a page turn fires until its screen arrives. */
  .pgspin { width:14px; height:14px; border:2px solid color-mix(in srgb, var(--brand) 30%, transparent);
    border-top-color:var(--brand); border-radius:50%; animation:pgspin .7s linear infinite; }
  @keyframes pgspin { to { transform:rotate(360deg); } }

  /* ── Reader (phone only) ───────────────────────────────────────────────
     The answer as words, not as a grid of cells.

     Paging the relay cannot be made smooth, and not because the link is slow:
     a full-screen TUI keeps no scrollback, so every page turn asks the CLI to
     scroll ITSELF and waits for a fresh screen — a round trip per third of a
     screen, on text already broken to the terminal's width and impossible to
     select. Here the phone holds the words instead. Scrolling is then the
     browser's own (a flick travels), lines re-wrap to the width, and anything
     on the page can be selected and copied.

     Which is why this is a plain document and not a second terminal: the one
     thing it must never become is another grid. */
  #reader { position:fixed; inset:0; z-index:30; display:none; flex-direction:column;
    background:var(--bg); color:var(--text); }
  #reader.on { display:flex; }
  #rhead { flex:0 0 auto; display:flex; align-items:center; gap:10px; padding:10px 12px;
    padding-top:calc(10px + env(safe-area-inset-top)); background:var(--panel);
    border-bottom:1px solid var(--line); }
  #rname { flex:1 1 auto; font-weight:700; min-width:0;
    overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
  #rclose { flex:none; width:34px; height:34px; border-radius:10px; border:1px solid var(--line);
    background:transparent; color:var(--text); font-size:16px; line-height:1; cursor:pointer;
    display:flex; align-items:center; justify-content:center; touch-action:manipulation; }
  #rbody { flex:1 1 auto; overflow-y:auto; overscroll-behavior:contain;
    -webkit-overflow-scrolling:touch;
    padding:16px 16px calc(32px + env(safe-area-inset-bottom));
    font-size:16px; line-height:1.75;
    user-select:text; -webkit-user-select:text; }
  /* The one place in this app where text is NOT monospace: this is prose to be
     read, and a proportional face fits more of it on a phone's width */
  #rbody, #rhead { font-family:system-ui, -apple-system, "Segoe UI", "Yu Gothic UI", sans-serif; }
  .rturn { margin:0 0 26px; }
  .rwho { font-size:11px; font-weight:700; letter-spacing:.09em; color:var(--dim);
    margin-bottom:6px; }
  /* What the person said is set back from what the AI answered: on a phone the
     eye needs the turn boundary more than it needs a bubble */
  .rturn.you { border-left:3px solid var(--line); padding-left:12px; color:var(--dim); }
  .rturn p { margin:0 0 12px; white-space:pre-wrap; overflow-wrap:anywhere; }
  .rturn h1, .rturn h2, .rturn h3 { font-size:1.05em; margin:18px 0 8px; }
  .rturn ul { margin:0 0 12px; padding-left:1.3em; }
  .rturn li { margin:0 0 6px; }
  .rturn code { font-family:ui-monospace, Consolas, monospace; font-size:.88em;
    background:var(--panel); border-radius:4px; padding:1px 4px; }
  /* A code block scrolls itself rather than widening the page. Long lines are
     the one thing that must not turn the whole reader into a horizontal pan */
  .rturn pre { margin:0 0 14px; padding:10px 12px; border-radius:8px; overflow-x:auto;
    background:var(--panel); border:1px solid var(--line); }
  .rturn pre code { background:none; padding:0; font-size:13px; line-height:1.5; }
  /* A table gets its own scroller for the same reason a code block does: on a
     phone it is nearly always wider than the screen, and the page itself must
     never be the thing that pans sideways */
  .rtable { overflow-x:auto; margin:0 0 14px; }
  .rturn table { border-collapse:collapse; font-size:14px; }
  .rturn th, .rturn td { border:1px solid var(--line); padding:6px 10px;
    text-align:left; vertical-align:top; }
  .rturn th { background:var(--panel); font-weight:700; white-space:nowrap; }
  /* The drawer handle is fixed to the top-left corner and would sit on top of
     the reader's own heading. While reading there is no board to reach for */
  body.reading #hamburger { display:none; }
  /* The way back into the past, at the head of the document — which is where
     the conversation carries on upward. Full width so a thumb cannot miss it,
     and quiet, because it is not the thing you came here to read */
  #rearlier { display:none; width:100%; margin:0 0 22px; padding:11px 12px;
    border:1px dashed var(--line); border-radius:10px; background:transparent;
    color:var(--dim); font:inherit; font-size:13px; cursor:pointer;
    touch-action:manipulation; }
  #rearlier:disabled { opacity:.6; }
  #rfoot { flex:0 0 auto; padding:8px 12px calc(8px + env(safe-area-inset-bottom));
    border-top:1px solid var(--line); color:var(--dim); font-size:12px; display:none; }
  #rfoot.on { display:block; }
  #rmore { position:absolute; left:50%; transform:translateX(-50%); bottom:18px;
    padding:8px 16px; border-radius:999px; border:1px solid var(--line);
    background:var(--brand); color:#fff; font-size:13px; font-weight:700;
    cursor:pointer; display:none; z-index:2; }
  #rmore.on { display:block; }

  /* ── Narrow/tall responsive layout (phones, small PCs, portrait displays) ──
     Must always come after all the base rules. Placed earlier, a later
     base rule at the same specificity would win and the override wouldn't stick */
  @media (max-width:700px), (max-aspect-ratio:1/1) {
    /* Never allow horizontal scroll (that mystery strip on the right), no matter what */
    html, body, #app { max-width:100vw; overflow-x:hidden; }
    /* Stack with flex instead of grid, to avoid a grid's "phantom second column" */
    #app { display:flex; flex-direction:column; }
    #main { flex:1; min-height:0; }

    /* Move the footer up into a top bar. The hamburger fits inside this bar,
       so it never overlaps any body content (the position:fixed ☰ sits in
       the bar's own left margin) */
    #status { order:-1; width:100vw; box-sizing:border-box; gap:8px;
      min-height:42px; padding-left:48px;
      border-top:none; border-bottom:1px solid var(--line); }
    /* ☰ sits on the left of the top bar (centered at the bar's height) */
    #hamburger { display:flex; top:6px; left:8px; }
    /* Save width on the emergency stop by showing just a red ■ (a universal stop symbol) */
    #stop { font-size:0; padding:2px 9px; }
    #stop::after { content:"\25A0"; font-size:13px; }
    /* Same trade for the restart button: the word goes, the circular arrow stays */
    #restart { font-size:0; padding:2px 9px; }
    #restart::after { content:"\21BB"; font-size:15px; }
    /* Hide the build stamp — it takes up space, and STOP must stay visible */
    #status .build { display:none; }

    /* Drawer-style tab bar */
    #tabs { position:fixed; top:0; left:0; bottom:0; z-index:30; width:240px;
      transform:translateX(-100%); transition:transform .2s ease;
      box-shadow:2px 0 14px rgba(0,0,0,.5);
      padding-top:46px; }   /* leaves room so the hamburger doesn't cover the first item */
    #app.drawer #tabs { transform:none; }
    #app.drawer #backdrop { display:block; position:fixed; inset:0; z-index:20;
      background:rgba(0,0,0,.45); }

    /* Body content sits below the top bar now, so it no longer needs margin for ☰ */
    #board { padding:16px 12px; }
    .card { overflow-x:auto; }          /* tables wider than the card scroll inside it */
    .menu { grid-template-columns:1fr; } /* single-column menu */
    /* The ASCII wordmark breaks up at this width, so switch to plain bold text */
    .mark { display:none; }
    .mark-lite { display:block; }
  }
</style>
<!-- Where a colour scheme chosen after this page was built lands. Empty
     until then, and a later rule of the same weight wins, so what starts in
     :root is simply replaced -->
<style id="theme"></style></head><body>
<div id="app">
  <div id="splash"><div class="logo">SHIKISHA-TERM</div><div class="spin"></div><div class="msg"></div></div>
  <div id="hamburger">&#9776;</div>
  <div id="backdrop"></div>
  <nav id="tabs"></nav>
  <!-- The tab bar's edge, as something you can take hold of -->
  <div id="tabgrip"></div>
  <div id="main">
    <div id="panes"></div>
    <div id="nav" hidden></div>
    <div id="page"></div>
    <div id="board" hidden></div>
    <!-- notranslate as well as the page-wide opt-out above: this is the one
         element a forced "translate this page" would wreck outright -->
    <pre id="screen" class="notranslate" translate="no" hidden></pre>
    <canvas id="cast" hidden></canvas>
    <div id="cur" hidden></div>
    <textarea id="kbd" autocomplete="off" autocorrect="off" spellcheck="false"></textarea>
    <pre id="probe">MMMMMMMMMM</pre>
    <pre id="tprobe"></pre>
    <div id="back" hidden></div>
    {{TOAST_HTML}}
    <div id="topicbar" hidden></div>
    <div id="thinking" hidden></div>
    <div id="pageui">
      <button id="readOpen" class="pagebtn" aria-label="read">&#128214;</button>
      <button id="pageUp" class="pagebtn" aria-label="older">&#9650;</button>
      <div id="pageCount"></div>
      <button id="pageDown" class="pagebtn" aria-label="newer">&#9660;</button>
    </div>
  </div>
  <div id="veil" hidden></div>
  <!-- The Vault: search past conversations and reopen one. Its own overlay
       rather than the veil, because it has an input and must not close on the
       first keystroke -->
  <!-- The command palette: one place to find and run anything. Same overlay
       shape as the Vault, a different list underneath -->
  <div id="palette" hidden>
    <div class="vbox">
      <div class="vhead"><span class="vtitle"></span><span class="vclose" title="close">✕</span></div>
      <input id="pq" type="text" autocomplete="off" spellcheck="false">
      <div class="vlist"></div>
    </div>
  </div>
  <div id="vault" hidden>
    <div class="vbox">
      <div class="vhead">
        <span class="vtitle"></span>
        <span class="vclose" title="close">✕</span>
      </div>
      <input id="vq" type="text" autocomplete="off" spellcheck="false">
      <div class="vhint"></div>
      <div class="vlist"></div>
    </div>
  </div>
  <!-- Another branch of a project already open. One thing to type; everything
       else is shown rather than asked -->
  <div id="branch" hidden>
    <div class="vbox">
      <div class="vhead"><span class="vtitle"></span><span class="vclose" title="close">✕</span></div>
      <div class="bsay"></div>
      <div class="brow2"><input id="bq" type="text" autocomplete="off" spellcheck="false"><select id="bbase"></select></div>
      <div class="bwhere"></div>
      <div class="bcmd"></div>
      <div class="bcarry"></div>
      <div class="berr"></div>
      <div class="brow"><button class="go"></button></div>
    </div>
  </div>
  <!-- Somewhere else to work. The same list on the window and on a phone -->
  <div id="browse" hidden>
    <div class="vbox">
      <div class="vhead"><span class="vtitle"></span><span class="vclose" title="close">&#10005;</span></div>
      <div class="bwhere"></div>
      <div class="berr"></div>
      <div class="vlist"></div>
      <div class="brow"><button class="go"></button></div>
    </div>
  </div>
  <!-- The reader: what was said on this tab, as text you can scroll and copy.
       Its own layer rather than a pane, because it covers the terminal and
       gives it straight back — nothing about the session changes while it is up -->
  <div id="reader">
    <div id="rhead">
      <div id="rname"></div>
      <button id="rclose" aria-label="close">✕</button>
    </div>
    <div id="rbody"></div>
    <div id="rfoot"></div>
    <button id="rmore"></button>
  </div>
  <div id="status"></div>
</div>
<script>
// Report failures that happen inside the page. If it dies silently, all
// anyone outside sees is "something that should have appeared didn't"
window.onerror = function (msg, src, line, col) {
  try {
    window.ipc.postMessage(JSON.stringify(
      {kind:"jserror", msg:String(msg) + " @" + line + ":" + col}));
  } catch (e) {}
};
const T = {{DICT}};
{ const _sm = document.querySelector("#splash .msg"); if (_sm) _sm.textContent = T["tui.splash.starting"] || ""; }
// If the splash lingers, say why instead of spinning silently. The long pole
// is WebView2 itself warming up (before this script even ran) plus the first
// board state; a cold first launch can take several seconds.
(function () {
  const say = (t) => {
    const s = document.getElementById("splash");
    const m = document.querySelector("#splash .msg");
    if (s && !s.hidden && m) m.textContent = t;
  };
  setTimeout(() => say(T["tui.splash.tabs"] || "Starting the tabs…"), 2000);
  setTimeout(() => say(T["tui.splash.slow"] || "Almost there — a cold start takes a little longer…"), 6000);
})();
const BUILD = {{BUILD}};
// The menu the dashboard shows. When a key is pressed, that character is delivered to INDEX as-is
const MENU_KEYS = {{MENU_KEYS}};
const MENU_WORDS = {{MENU_WORDS}};
// Menu entries only the window can carry out (the remote gate refuses them too).
const MENU_WINDOW_ONLY = {{MENU_WINDOW_ONLY}};
// Menu entries the board carries out itself rather than forwarding as a keystroke.
// Keyed by the entry's translation key, so the letter on the button can change
// without this quietly falling back to the keystroke path.
const MENU_OWN = {"tui.menu.settings": () => openSettings(), "tui.menu.vault": () => window.__openVault(), "tui.menu.palette": () => window.__openPalette()};
// The auxiliary key row shown in the screen relay (customizable via config)
const CAST_KEYS = {{CAST_KEYS}};
// Quick actions for the sub-input bar: [{label, text, lua}]. text!=null = insert
// that string on click (beginner); lua=true = fire server-side Lua (advanced).
// ACTIONS is the value baked in at page load; curActions is the live copy the bar
// renders, swapped by __setActions when the config changes (so a settings edit
// reflects without reloading the window).
const ACTIONS = {{ACTIONS}};
// Every rebindable action, as {name, label}. The palette runs one by name;
// the label is the translated description a person reads
const KEY_ACTIONS = {{KEY_ACTIONS}};
let curActions = (typeof ACTIONS !== "undefined" && ACTIONS) ? ACTIONS : [];
// The access token is never baked into the page. On first pair it rides in the
// URL (?t=…, straight from the QR); we lift it into sessionStorage and then
// immediately strip it from the address bar and this history entry with
// history.replaceState — so the token stops lingering where it can leak: browser
// history, an autocompleted address bar, a bookmark, cross-device sync, a glance
// over the shoulder. A reload finds it again in sessionStorage (survives reload,
// cleared when the tab closes); a fresh tab has none and must re-pair from the QR.
// The window (not REMOTE) loads over its own loopback origin with no ?t= and no
// sessionStorage, so TOKEN is "" there and never used.
// STICKY (config remote.sticky_token) trades that caution for a pairing that
// survives: the token stays in the URL (so the page can be bookmarked and a
// discarded mobile tab comes straight back) and in localStorage (a bookmark
// without ?t= still signs in). The person opted into that trade knowingly.
const STICKY = {{STICKY}};
const TOKEN = (function () {
  try {
    const t = new URLSearchParams(location.search).get("t");
    if (STICKY) {
      // Switching the mode off must also stop the lingering copy — handled by
      // the non-sticky branch below, which clears localStorage on every load
      if (t) { try { localStorage.setItem("shikisha_token", t); } catch (e) {} return t; }
      return localStorage.getItem("shikisha_token") || "";
    }
    try { localStorage.removeItem("shikisha_token"); } catch (e) {}
    if (t) {
      try { sessionStorage.setItem("shikisha_token", t); } catch (e) {}
      try { history.replaceState({}, "", location.pathname); } catch (e) {}
      return t;
    }
    return sessionStorage.getItem("shikisha_token") || "";
  } catch (e) { return ""; }
})();
// Inside the window, messages can be handed over directly. From a phone
// they arrive over HTTP. Both use the same page (so the UI isn't written twice)
const REMOTE = !window.ipc;
// The PC ended this session (its "disconnect"). Nothing reconnects afterwards —
// not the state socket, not the screen relay — until a person opens the link
// again, which reloads this page and clears the flag with it.
let remoteCut = false;
const send = REMOTE
  ? (o => fetch("api/intent?t=" + encodeURIComponent(TOKEN),
      {method:"POST", body:JSON.stringify(o)}).catch(() => {}))
  : (o => window.ipc.postMessage(JSON.stringify(o)));
const el = (t, a, ...kids) => {
  const n = document.createElement(t);
  for (const k in (a||{})) {
    if (k === "class") n.className = a[k];
    else if (k.startsWith("on")) n[k] = a[k];
    else if (a[k] !== null && a[k] !== undefined) n.setAttribute(k, a[k]);
  }
  for (const c of kids) if (c !== null && c !== undefined) n.append(c);
  return n;
};

let S = null;   // most recent state
// The message the app last told us about. Kept so an unchanged one isn't shown again
let lastFlash = null;

// The shared toast (src/toast.rs). Declared this early because the very first
// state can arrive with a message already in it.
{{TOAST_JS}}
// Where the toast sits in this window. The composer bar owns the bottom edge
// while it's open — and rises with the phone's on-screen keyboard — so the
// toast stands on top of it instead of over the thing being typed into.
function toastBottom() {
  const main = document.getElementById("main");
  if (!main || !castDock || castDock.style.display !== "flex") return "52px";
  const m = main.getBoundingClientRect(), d = castDock.getBoundingClientRect();
  return Math.max(52, Math.round(m.bottom - d.top) + 14) + "px";
}

// Don't rebuild the DOM while a press is in progress.
// A click only registers when it "starts and ends on the same element".
// Rebuilding the dashboard between pointerdown and pointerup means the
// pressed element no longer exists, so that press never reaches anywhere.
// The activity graph keeps redrawing constantly, so this wasn't some rare
// edge case — it was the default behavior.
let holding = false, queued = null, holdTimer = 0;
const release = () => {
  holding = false;
  clearTimeout(holdTimer);
  if (queued === null) return;
  const j = queued; queued = null;
  // Hand the held-back redraw to the NEXT task rather than running it here.
  // A click has not been delivered yet at pointerup — the browser dispatches it
  // immediately afterwards — so rebuilding from this handler removes the very
  // element the click was about to land on, and the press vanishes with it.
  // That is the same "started and ended on the same element" rule the guard
  // above exists for; it simply has to hold one step longer than the press.
  setTimeout(() => window.__state(j), 0);
};
addEventListener("pointerdown", () => {
  holding = true;
  // The "released" signal can fail to arrive. If a finger lifts off over a
  // page layered on top, pointerup never reaches us here. Left stuck in a
  // "still pressed" state, the screen would never redraw again and tabs
  // would look like they stopped responding.
  // One second is plenty to guard a press.
  clearTimeout(holdTimer);
  holdTimer = setTimeout(release, 1000);
}, true);
addEventListener("pointerup", release, true);
addEventListener("pointercancel", release, true);
// Covers the case where the pointer is released outside the window — better than
// staying stuck "held". The window's own blur and nothing else: with capture this
// caught EVERY element's blur, and clicking a tab blurs whatever had focus, so the
// guard was disarmed in the middle of the very press it exists to protect — the
// next redraw then took the element out from under the click and the press was
// lost. blur does not bubble, so a plain listener here hears only the window.
addEventListener("blur", release);

// Drawer-style tab bar (narrow/tall screens). Inert on wide screens since the bar stays visible there
{
  const app = document.getElementById("app");
  const ham = document.getElementById("hamburger");
  const bd = document.getElementById("backdrop");
  if (ham) ham.onclick = () => app.classList.toggle("drawer");
  if (bd) bd.onclick = () => app.classList.remove("drawer");
  // Picking a tab collapses the drawer back to full width
  const tabs = document.getElementById("tabs");
  if (tabs) tabs.addEventListener("click", () => app.classList.remove("drawer"));
  // Pressing the top bar (outside the cast area) exits control mode
  const st = document.getElementById("status");
  if (st) st.addEventListener("pointerdown", () => { if (typeof exitCast === "function") exitCast(); });
}

// ── Left tab bar ────────────────────────────
function drawTabs() {
  const nav = document.getElementById("tabs");
  nav.textContent = "";
  // Above INDEX: the current workspace and a switcher. Clicking opens the list popup
  nav.append(el("div", {class:"tab wsrow", title:T["tui.menu.workspace"] || "WORKSPACE",
      onclick:() => send({kind:"openws"})},
    el("span", {class:"num"}, "◇"),
    el("span", {class:"nm"}, S.workspace || ""),
    el("span", {class:"wscaret"}, "▾")));
  nav.append(el("div", {class:"tab" + (S.board ? " sel" : ""),
      onclick:() => send({kind:"select", tab:0})},
    el("span", {class:"num"}, "0"),
    el("span", {class:"nm"}, T["tui.index"] || "INDEX")));
  // The folder each run of tabs works in. A heading appears when the folder
  // changes, and only when there is more than one to change to -- with a single
  // folder the sidebar looks exactly as it always has. A tab that is in no
  // folder at all (a browser) leaves the heading alone rather than ending it,
  // so a page declared between two tabs does not split their folder in two
  const folders = S.groups || [];
  let shownFolder = -1;
  for (const t of S.tabs) {
    // Settings isn't a tab — it's reached via the gear pinned at the bottom.
    if (t.settings) continue;
    if (folders.length > 1 && t.group != null && t.group !== shownFolder) {
      shownFolder = t.group;
      const g = folders[t.group] || {};
      const shut = folded.has(g.folder);
      const chip = el("span", {class:"chip"});
      if (g.color) chip.style.background = g.color;
      nav.append(el("div", {class:"tab folder" + (g.linked ? " cut" : ""),
          title:g.folder || "", onclick:() => { fold(g.folder); }},
        g.linked ? el("span", {class:"hang"}, "\u2514") : null,
        chip,
        el("span", {class:"caret"}, shut ? "▸" : "▾"),
        // A branch cut from the project it belongs to, rather than the
        // project's own folder. Worth marking: closing one is a different act.
        // Drawn rather than typed — every character that means "branch" is one
        // some font has never heard of, and the fallback is a shrug
        g.linked ? cutMark() : null,
        el("span", {class:"nm"}, g.name || ""),
        el("span", {class:"more", title:T["tui.folder.more"] || "…",
            onclick:e => { e.stopPropagation(); folderMenu(e, g); }}, "⋯")));
    }
    // Its tabs are hidden while it is folded, and the heading says so
    if (t.group != null && folders.length > 1 && folded.has((folders[t.group] || {}).folder)) continue;
    // Running several AIs side by side is the headline feature, so brand each
    // AI tab in its own colour (a left bar + a tinted name). The status dot
    // stays separate — colour = which AI, dot = what it's doing. Nothing is
    // inserted before the dot, so every row's dot sits at the same x and the
    // column reads as one line down the sidebar.
    const brand = t.ai ? " aitab ai-" + t.ai : "";
    // A branch's tabs stand where its heading does. Moving the heading alone
    // left them looking like they belonged to the folder above it
    const under = (t.group != null && (folders[t.group] || {}).linked) ? " under" : "";
    nav.append(el("div", {class:"tab" + (S.active === t.index ? " sel" : "") + brand + under,
        onclick:() => send({kind:"select", tab:t.index})},
      el("span", {class:"dot " + t.state}),
      el("span", {class:"num"}, String(t.index)),
      el("span", {class:"nm", title:t.profile}, t.name),
      t.locked ? el("span", {class:"lock"}, "\u{1F512}") : null,
      spark(t.activity)));
    // What it says it is doing, under its name. Only when it has said
    // something: an empty second line on every tab would spend half the
    // sidebar saying nothing
    // Where it is, then what it last said. Each only when there is one: a
    // blank line on every tab would spend the sidebar saying nothing
    if (t.place) {
      const p = t.place;
      const line = el("span", {class:"place"});
      // Not when the heading right above already says it. Two tabs under
      // "feature/login" saying "feature/login" each is the sidebar spending
      // three lines on one fact
      const heading = (t.group != null && folders.length > 1) ? (folders[t.group] || {}).name : null;
      if (p.branch && p.branch !== heading) {
        // A long branch name is shortened from the front. The end of a branch
        // name is the part someone chose ("…/fix-login"); the front is the
        // part a tool prepended, and cutting the tail throws away the half
        // that says which branch this is
        const short = p.branch.length > 28 ? "…" + p.branch.slice(-27) : p.branch;
        line.append(el("span", {class:"br"}, short));
      }
      if (p.pr) line.append(el("span", {class:"pr"}, p.pr));
      for (const port of (p.ports || [])) {
        line.append(el("span", {class:"pt"}, ":" + port));
      }
      // The whole of it on hover, since the row cannot hold it all
      line.title = [p.branch, p.pr].filter(Boolean)
        .concat((p.ports || []).map(x => ":" + x)).join("  ");
      nav.lastChild.append(line);
    }
    if (t.status) {
      nav.lastChild.append(el("span", {class:"said", title:t.status}, t.status));
    }
  }
  // A "+" at the end of the list. Opens the settings page already in the "add tab" state
  nav.append(el("div", {class:"tab addtab", onclick:e => addMenu(e)},
    el("span", {class:"num"}, "+"),
    el("span", {class:"nm"}, T["tui.tab.add"] || "ADD TAB")));
  // The settings gear, pinned to the very bottom of the sidebar. Always visible.
  const settingsOpen = !!S.settings_open;
  nav.append(el("div", {class:"tab gearrow" + (settingsOpen ? " sel" : ""),
      title:T["tui.menu.settings"] || "SETTINGS",
      // On the phone, settings is served by reverse-proxy at /cfg and rendered
      // natively (responsive) — navigate there, handing over the token once in
      // the URL (it's traded for a cookie and stripped on arrival). In the window
      // it opens as the child WebView, as before.
      onclick:() => openSettings()},
    el("span", {class:"gear"}, "⚙️")));
}

// Folders whose tabs are put away for now. Kept here rather than in the app:
// how much of a list is on screen is this screen's business, and the phone and
// the window are each looking at their own
const folded = new Set();
function fold(folder) {
  if (!folder) return;
  folded.has(folder) ? folded.delete(folder) : folded.add(folder);
  drawNav();
}

// What a folder can do. One menu, opened by pointer or by finger -- nothing
// here is reachable only by hovering, because half the people using it are on
// a phone
function folderMenu(e, g) {
  closeFolderMenu();
  const item = (label, go) => el("div", {onclick:() => { closeFolderMenu(); go(); }}, label);
  const m = el("div", {class:"fmenu"},
    item(T["tui.tab.add"] || "ADD TAB", addTabHere),
    // Only where there is a project to cut a branch from
    g.color ? item(T["tui.folder.branch"] || "Parallel work (git worktree)",
                   () => openBranch(g)) : null,
    g.color ? colorItem(g) : null,
    item(folded.has(g.folder) ? (T["tui.folder.open"] || "Unfold")
                              : (T["tui.folder.fold"] || "Fold"),
         () => fold(g.folder)),
    renameItem(g),
    closeItem(g),
    g.linked ? discardItem(g) : null);
  document.body.append(m);
  // Below what was pressed, and never off the bottom of the window
  const r = e.currentTarget.getBoundingClientRect();
  const box = m.getBoundingClientRect();
  m.style.left = Math.min(r.left, window.innerWidth - box.width - 8) + "px";
  m.style.top = Math.min(r.bottom + 4, window.innerHeight - box.height - 8) + "px";
  // A press anywhere else puts it away -- but a press *on it* must not, or the
  // menu would be gone before the release that makes the click, and every
  // entry would look dead
  folderMenuAway = ev => { if (!m.contains(ev.target)) closeFolderMenu(); };
  setTimeout(() => document.addEventListener("mousedown", folderMenuAway, true), 0);
}
let folderMenuAway = null;
// The three ways a workspace grows, said as what happens rather than as what
// they are. Parallel work only appears where there is a project to cut a
// branch from, so someone with no repository never meets the idea
function addMenu(e) {
  closeFolderMenu();
  const item = (label, go) => el("div", {onclick:() => { closeFolderMenu(); go(); }}, label);
  // The folder of the tab being looked at, so "another branch" means this one
  const at = (S && S.tabs || []).find(t => t.index === S.active);
  const gs = (S && S.groups) || [];
  const here = (at && at.group != null && gs[at.group] && gs[at.group].color)
    ? gs[at.group] : gs.find(g => g.color) || null;
  const m = el("div", {class:"fmenu"},
    item(T["tui.tab.add"] || "ADD TAB", addTabHere),
    here ? item(T["tui.folder.branch"] || "Parallel work (git worktree)",
                () => openBranch(here)) : null,
    item(T["tui.folder.another"] || "Open another folder", () => openBrowse("")));
  document.body.append(m);
  const r = e.currentTarget.getBoundingClientRect();
  const box = m.getBoundingClientRect();
  m.style.left = Math.min(r.left, window.innerWidth - box.width - 8) + "px";
  m.style.top = Math.min(r.bottom + 4, window.innerHeight - box.height - 8) + "px";
  folderMenuAway = ev => { if (!m.contains(ev.target)) closeFolderMenu(); };
  setTimeout(() => document.addEventListener("mousedown", folderMenuAway, true), 0);
}

// Somewhere else to work. The list comes from the app, so this is the same
// walk from the window and from a phone -- there is no dialog the operating
// system can draw on a phone, and one list is one thing to keep right
function openBrowse(at) {
  const b = document.getElementById("browse");
  if (!b) return;
  b.hidden = false;
  b.querySelector(".vtitle").textContent = T["tui.browse.title"] || "OPEN A FOLDER";
  b.querySelector(".go").textContent = T["tui.browse.open"] || "Open";
  send({kind:"browse", path:at || "", open:false});
}
function closeBrowse() {
  const b = document.getElementById("browse");
  if (b) b.hidden = true;
}
function drawBrowse() {
  const b = document.getElementById("browse");
  if (!b || b.hidden) return;
  const st = (S && S.browse) || null;
  b.querySelector(".bwhere").textContent = (st && st.at) || (T["tui.browse.top"] || "");
  b.querySelector(".berr").textContent = (st && st.error) || "";
  const list = b.querySelector(".vlist");
  list.textContent = "";
  if (st && st.up != null) {
    list.append(el("div", {class:"vrow", onclick:() => openBrowse(st.up)},
      el("span", {class:"nm"}, "..")));
  }
  for (const d of (st && st.dirs) || []) {
    // The last part is what a person reads; the whole path is the tooltip
    const leaf = d.replace(/[\\/]+$/, "").split(/[\\/]/).pop() || d;
    list.append(el("div", {class:"vrow", title:d, onclick:() => openBrowse(d)},
      el("span", {class:"nm"}, leaf)));
  }
  b.querySelector(".go").disabled = !(st && st.at);
}
(function () {
  const b = document.getElementById("browse");
  if (!b) return;
  b.querySelector(".vclose").onclick = closeBrowse;
  b.addEventListener("mousedown", e => { if (e.target === b) closeBrowse(); });
  b.querySelector(".go").onclick = () => {
    const st = (S && S.browse) || null;
    if (!st || !st.at) return;
    closeBrowse();
    send({kind:"browse", path:st.at, open:true});
  };
})();

// Renaming: the row becomes the field, so there is nothing else to open and
// nowhere else to look. Empty hands the heading back to what the folder says
// about itself -- its branch, or its own last part
function renameItem(g) {
  const row = el("div", {}, T["tui.folder.rename"] || "Rename");
  row.onclick = e => {
    e.stopPropagation();
    row.textContent = "";
    const inp = el("input", {class:"fname", value:g.name || "",
                             placeholder:T["tui.folder.rename.ph"] || ""});
    inp.addEventListener("keydown", ev => {
      ev.stopPropagation();
      if (ev.key === "Enter") {
        closeFolderMenu();
        send({kind:"foldername", folder:g.folder, name:inp.value});
      }
      if (ev.key === "Escape") closeFolderMenu();
    });
    inp.addEventListener("mousedown", ev => ev.stopPropagation());
    row.append(inp);
    setTimeout(() => { inp.focus(); inp.select(); }, 0);
  };
  return row;
}

// Closing: the tabs go, the files stay. Asked twice, because the tabs may be
// in the middle of something and there is no putting that back
function closeItem(g) {
  const row = el("div", {class:"warn"}, T["tui.folder.close"] || "Close");
  let armed = false;
  row.onclick = e => {
    e.stopPropagation();
    if (!armed) {
      armed = true;
      row.textContent = T["tui.folder.close.sure"] || "Close it?";
      return;
    }
    closeFolderMenu();
    send({kind:"folderclose", folder:g.folder});
  };
  return row;
}

// Throwing the folder away for good. Only offered for a branch's own folder,
// asked twice like closing, and the app refuses it outright while there is
// anything in there that is not committed -- the answer comes back as the
// message, not as a folder that is already gone
function discardItem(g) {
  const row = el("div", {class:"warn"}, T["tui.folder.discard"] || "Delete the folder");
  let armed = false;
  row.onclick = e => {
    e.stopPropagation();
    if (!armed) {
      armed = true;
      row.textContent = T["tui.folder.discard.sure"] || "Delete it?";
      return;
    }
    closeFolderMenu();
    send({kind:"folderdiscard", folder:g.folder});
  };
  return row;
}

// The colours for a project, offered as squares. Picking one is the whole
// interaction -- there is nothing to confirm, and the list is repainted from
// the app's answer, so what is on screen is what was actually saved
function colorItem(g) {
  const row = el("div", {}, T["tui.folder.color"] || "Colour");
  const box = el("div", {class:"swatches"});
  const pick = c => { closeFolderMenu(); send({kind:"foldercolor", folder:g.folder, color:c}); };
  for (const c of ["#d97757","#19c37d","#4285f4","#a06bff",
                   "#e0a80a","#12b3a8","#e5644d","#7f8cff"]) {
    const sw = el("i", {onclick:e => { e.stopPropagation(); pick(c); }});
    sw.style.background = c;
    box.append(sw);
  }
  // Anything at all, through the picker the system already has
  const any = el("input", {type:"color", value:g.color || "#888888"});
  any.addEventListener("input", () => pick(any.value));
  const opener = el("i", {class:"any", onclick:e => { e.stopPropagation(); any.click(); }});
  box.append(opener, any);
  row.append(box);
  row.onclick = e => e.stopPropagation();
  return row;
}

function closeFolderMenu() {
  if (folderMenuAway) {
    document.removeEventListener("mousedown", folderMenuAway, true);
    folderMenuAway = null;
  }
  for (const m of document.querySelectorAll(".fmenu")) m.remove();
}

// Another branch of the project this folder belongs to.
//
// One thing to type. Where the folder will be and the command that will make
// it are answered by the app -- the same code that will run it -- and shown
// while the name is still being typed
let branchFrom = "";
let branchTimer = 0;
function openBranch(g) {
  const b = document.getElementById("branch");
  if (!b) return;
  branchFrom = g.folder || "";
  b.hidden = false;
  b.querySelector(".vtitle").textContent = T["tui.branch.title"] || "PARALLEL WORK";
  b.querySelector(".bsay").textContent = T["tui.branch.hint"] || "";
  b.querySelector(".go").textContent = T["tui.branch.make"] || "Make it";
  const q = document.getElementById("bq");
  q.placeholder = T["tui.branch.placeholder"] || "branch name";
  q.value = "";
  const box = b.querySelector(".bcarry");
  box.dataset.key = "";
  box.textContent = "";
  const sel = document.getElementById("bbase");
  sel.dataset.key = "";
  sel.textContent = "";
  drawBranch();
  // Asked before a single letter is typed: what this project can be grown
  // from does not depend on the name, and a picker that is empty until you
  // type is a picker nobody finds anything in
  send({kind:"branch", from:branchFrom, branch:"", base:"", make:false, carry:[]});
  setTimeout(() => q.focus(), 30);
}
function closeBranch() {
  const b = document.getElementById("branch");
  if (b) b.hidden = true;
  branchFrom = "";
}
// Asks what would happen. Not on every letter -- a name is typed in bursts,
// and a question per keystroke would answer about halves of words
function askBranch() {
  clearTimeout(branchTimer);
  branchTimer = setTimeout(() => {
    const q = document.getElementById("bq");
    send({kind:"branch", from:branchFrom, branch:(q ? q.value : ""), base:basing(),
          make:false, carry:carrying()});
  }, 180);
}

// What it will grow from: whatever is showing in the picker
function basing() {
  const sel = document.getElementById("bbase");
  return sel ? sel.value : "";
}

// The names still ticked, in the order they were offered
function carrying() {
  const b = document.getElementById("branch");
  if (!b) return [];
  return Array.from(b.querySelectorAll(".bcarry input:checked")).map(i => i.value);
}
function drawBranch() {
  const b = document.getElementById("branch");
  if (!b || b.hidden) return;
  const p = (S && S.branch) || null;
  const typed = (document.getElementById("bq") || {}).value || "";
  // An answer about a name that has since changed says nothing about this one
  const mine = p && p.from === branchFrom && p.branch === typed.trim();
  // The lists belong to the folder, not to the name: an answer for this folder
  // fills them whatever was typed when it was asked
  const here = p && p.from === branchFrom;
  if (mine && p.done) { closeBranch(); return; }
  b.querySelector(".bwhere").textContent = mine && !p.error ? p.folder : "";
  b.querySelector(".bcmd").textContent = mine && !p.error ? p.line : "";
  drawCarry(b, here ? (p.carry || []) : []);
  drawBases(b, here ? p : null);
  b.querySelector(".berr").textContent = mine && p.error ? p.error : "";
  b.querySelector(".go").disabled = !(mine && !p.error);
}
// The branches this one can grow from. Filled once, then left alone: rebuilt
// on every answer it would jump back to the first one each time somebody
// chose another
function drawBases(b, p) {
  const sel = document.getElementById("bbase");
  if (!sel) return;
  const list = (p && p.bases) || [];
  // An answer that has nothing to say about the branches -- one about a name
  // that has since changed -- must not empty a picker that is already filled,
  // least of all while it is open
  if (!list.length && sel.options.length) return;
  const key = list.join("\u0000");
  if (sel.dataset.key === key) return;
  sel.dataset.key = key;
  sel.textContent = "";
  for (const name of list) {
    const said = (T["tui.branch.from"] || "from {name}").replace("{name}", name);
    sel.append(el("option", {value:name}, said));
  }
  if (p && p.base) sel.value = p.base;
  // Asking again with the chosen one, so the command line below follows
  sel.onchange = askBranch;
}

// Drawn once per set of names: rebuilding it on every answer would untick
// whatever was just unticked, which is the one thing this list must not do
function drawCarry(b, items) {
  const box = b.querySelector(".bcarry");
  if (!items.length && box.children.length) return;
  const key = items.map(i => i.name).join("\u0000");
  if (box.dataset.key === key) return;
  box.dataset.key = key;
  box.textContent = "";
  if (!items.length) return;
  box.append(el("span", {class:"say"}, T["tui.branch.carry"] || "Bring along:"));
  for (const it of items) {
    const cb = el("input", {type:"checkbox", value:it.name});
    cb.checked = !!it.on;
    box.append(el("label", {title:it.folder ? (T["tui.branch.carry.link"] || "") : ""},
      cb, el("span", {}, it.name)));
  }
}

(function () {
  const b = document.getElementById("branch");
  if (!b) return;
  b.querySelector(".vclose").onclick = closeBranch;
  b.addEventListener("mousedown", e => { if (e.target === b) closeBranch(); });
  b.querySelector(".go").onclick = () => {
    const q = document.getElementById("bq");
    send({kind:"branch", from:branchFrom, branch:(q ? q.value : ""), base:basing(),
          make:true, carry:carrying()});
  };
  const q = document.getElementById("bq");
  q.addEventListener("input", askBranch);
  q.addEventListener("keydown", e => {
    if (e.key === "Escape") { e.preventDefault(); closeBranch(); }
    // Enter makes it, but only once the app has said it can be made
    if (e.key === "Enter" && !b.querySelector(".go").disabled) {
      e.preventDefault();
      b.querySelector(".go").click();
    }
  });
})();

// The mark on a folder that is a branch cut from another: a line, and a second
// one leaving it. Takes its colour from the row it sits in
function cutMark() {
  const s = el("span", {class:"cut"});
  s.innerHTML = '<svg viewBox="0 0 12 12" width="11" height="11" fill="none" ' +
    'stroke="currentColor" stroke-width="1.3" stroke-linecap="round">' +
    '<path d="M3.2 2.6v6.8"/><path d="M3.2 6.4c0-2.2 5.4-1 5.4-3.2"/>' +
    '<circle cx="3.2" cy="10" r="1.2" fill="currentColor" stroke="none"/>' +
    '<circle cx="8.6" cy="2.4" r="1.2" fill="currentColor" stroke="none"/></svg>';
  return s;
}

// Draw output volume as a real bar chart, not ▁▄█ characters
function spark(a) {
  const box = el("div", {class:"spark"});
  for (const v of (a || []).slice(-10)) {
    const b = el("i");
    b.style.height = Math.max(1, v * 2) + "px";
    box.append(b);
  }
  return box;
}

// ── Dashboard ──────────────────────────────────
const WORDMARK = [
  "█▀▀ █ █ █ █ █ █ █▀▀ █ █ █▀█    ▀█▀ █▀▀ █▀█ █▄█",
  "▀▀█ █▀█ █ █▀▄ █ ▀▀█ █▀█ █▀█ ▀▀  █  █▀▀ █▀▄ █ █",
  "▀▀▀ ▀ ▀ ▀ ▀ ▀ ▀ ▀▀▀ ▀ ▀ ▀ ▀     ▀  ▀▀▀ ▀ ▀ ▀ ▀",
];

function drawBoard() {
  const b = document.getElementById("board");
  b.textContent = "";
  // Wide screens get the ASCII-art wordmark. Narrow screens break it up,
  // so they switch to plain bold text (handled via CSS media query)
  b.append(el("div", {class:"mark"}, WORDMARK.join("\n")),
           el("div", {class:"mark-lite"}, "SHIKISHA-TERM"),
           el("div", {class:"sub"},
             el("span", {class:"wslink", title:T["tui.menu.workspace"] || "WORKSPACE",
               onclick:() => send({kind:"openws"})}, S.workspace || ""),
             el("span", {}, "   " + BUILD),
             // What this app is costing the machine, all in -- the terminal,
             // the agents it launched, and the browser it embeds. Its own
             // weight, said plainly rather than left for a task manager to
             // reveal. Only when there is a figure to show
             S.self_cost ? el("span", {class:"selfcost", title:T["tui.self_cost"] || "this app, all processes"},
               "   " + S.self_cost) : null));

  // Chain
  const heat = S.ball.max ? Math.min(1, S.ball.depth / S.ball.max) : 0;
  const bar = el("i");
  bar.style.width = Math.round(heat * 100) + "%";
  bar.style.background = heat >= .8 ? "var(--stop)" : heat >= .5 ? "var(--warn)" : "var(--live)";
  b.append(el("div", {class:"card"},
    el("h2", {}, T["tui.chain"] || "CHAIN"),
    el("div", {class:"gauge"}, bar),
    el("div", {class:"sub"}, S.ball.depth + " / " + S.ball.max),
    lanes()));

  // (The "start the discussion" prompt used to live here as a dashboard card,
  // but it blended into the board and only showed on INDEX. It's now the
  // #topicbar banner — floated over every tab while the discussion is at rest.)

  // Tab list
  const rows = el("table", {class:"rows"});
  rows.append(el("tr", {},
    el("th", {}, "#"), el("th", {}, T["tui.col.name"] || "NAME"),
    el("th", {}, T["tui.col.state"] || "STATE"),
    el("th", {}, T["tui.col.profile"] || "PROFILE"),
    el("th", {}, T["tui.col.cost"] || "COST"),
    el("th", {}, T["tui.col.activity"] || "ACTIVITY")));
  for (const t of S.tabs) {
    rows.append(el("tr", {onclick:() => send({kind:"select", tab:t.index})},
      el("td", {}, String(t.index)),
      el("td", {}, t.name),
      el("td", {}, el("span", {class:"dot " + t.state}), " " + (t.state_label || t.state)),
      el("td", {}, t.profile),
      // What it is costing the machine. Blank for a tab that costs nothing --
      // a browser, an idle shell -- so the eye lands on the ones that do
      el("td", {class:"cost"}, t.cost || ""),
      el("td", {}, spark(t.activity))));
  }
  b.append(el("div", {class:"card"}, el("h2", {}, "SESSIONS"), rows));

  // Menu. Order is decided by MENU_KEYS (to keep it in sync with the receiver)
  const items = MENU_KEYS.map(k => [k, T[MENU_WORDS[k]]]);
  const m = el("div", {class:"menu"});
  for (const [k, label] of items) {
    if (!label) continue;
    // An entry the shell performs itself beats a keystroke: settings has a real
    // destination on both surfaces (the window's child WebView, the phone's own
    // /cfg page), and a bare 'e' would only ever reach the window.
    const own = MENU_OWN[MENU_WORDS[k]];
    const stuck = REMOTE && !own && MENU_WINDOW_ONLY.includes(k);
    m.append(el("div", {class:"mi" + (stuck ? " windowonly" : ""),
        title: stuck ? T["tui.menu.window_only"] : label,
        onclick: stuck ? null : (own || (() => send({kind:"menu", key:k})))},
      el("span", {class:"key"}, k), el("span", {}, label),
      stuck ? el("span", {class:"only"}, T["tui.menu.window_only"]) : null));
  }
  b.append(el("div", {class:"card"}, el("h2", {}, "MENU"), m));
}

// The ball's track. Lays out the human (0) and each tab, and actually moves the circle
function lanes() {
  const box = el("div", {id:"lanes"});
  const n = S.tabs.length + 1;
  const line = el("div", {class:"lane"});
  line.style.left = "6%"; line.style.right = "6%";
  box.append(line);
  const at = i => 6 + (n <= 1 ? 44 : (i * 88) / (n - 1));
  for (let i = 0; i < n; i++) {
    const p = el("div", {class:"peg"},
      el("b", {}, i === 0 ? (T["tui.human"] || "YOU") : S.tabs[i-1].name));
    p.style.left = at(i) + "%";
    box.append(p);
  }
  const ball = el("div", {id:"ball"});
  ball.style.left = at(S.ball.holder) + "%";
  ball.style.top = "50%";
  if (S.ball.holder === 0) ball.className = "human";
  if (S.ball.awaiting_human) ball.className += " wait";
  box.append(ball);
  return box;
}

// ── Discussion topic banner ──────────────────
// A prominent prompt floated over any tab while an AI-vs-AI discussion is at
// rest. Type a topic → it's sent to the opening speaker and the round starts.
// The banner is rebuilt only when the target changes (workspace / speaker),
// so a half-typed topic survives the frequent state pushes.
function drawTopicBar() {
  const bar = document.getElementById("topicbar");
  const show = !!(S && S.discuss_start && S.discuss_idle);
  if (!show) { bar.hidden = true; return; }
  const sig = (S.workspace || "") + "|" + S.discuss_start + "|" + (S.discuss_start_name || "");
  if (bar.dataset.sig === sig && bar.childNodes.length) { bar.hidden = false; return; }
  bar.dataset.sig = sig;
  bar.textContent = "";
  const input = el("input", {type:"text",
    placeholder: T["tui.discuss.start.ph"] || "Type the topic to start the discussion"});
  const go = () => {
    const topic = input.value.trim();
    if (!topic) { input.focus(); return; }
    // Put the opening speaker in front, then hand it the topic the way it
    // takes input. Typed as keystrokes it reached a CLI and vanished into a
    // model bridge, which has no keyboard to type at -- so a discussion whose
    // opening speaker was a model could not be started at all.
    send({kind:"select", tab:S.discuss_start});
    sendLine(topic, S.discuss_start);
    input.value = "";
    // It vanishes on its own once the opening speaker goes BUSY, but hide it
    // right away so a stray second Enter can't fire a duplicate topic.
    bar.hidden = true;
  };
  input.addEventListener("keydown", e => { if (e.key === "Enter") { e.preventDefault(); go(); } });
  const hint = (T["tui.discuss.start.hint"] || "Sends your topic to the opening speaker ({name}) and begins.")
    .split("{name}").join(S.discuss_start_name || "");
  bar.append(
    el("span", {class:"tb-ico"}, "\u{1F5E3}"),
    el("span", {class:"tb-label"}, T["tui.discuss.start.title"] || "Start the discussion"),
    input,
    el("button", {onclick:go}, T["tui.discuss.start.btn"] || "Start"),
    el("span", {class:"tb-hint"}, hint));
  bar.hidden = false;
}

// A model pane types into the same sub-input bar as everything else -- see
// sendBar(), which is the one place that decides where a Send goes. All that is
// left here is the waiting indicator.
//
// The thinking bubble's live elapsed timer. __state only fires on change, so a
// standalone interval keeps the seconds ticking while the reply generates.
let thinkTimer = null, thinkStart = 0;
function fmtElapsed(ms) {
  const s = Math.floor(ms / 1000);
  return s < 60 ? s + "s" : Math.floor(s / 60) + "m " + (s % 60) + "s";
}
// Claude-style bouncing dots parked where the reply will appear, so a wait on a
// model pane never looks like a pane that has died. Toggled in place -- the
// bubble is not rebuilt on every state push, or its elapsed timer would restart
// each frame.
function drawThinking() {
  const think = document.getElementById("thinking");
  const t = activeTab();
  if (!(t && t.model && t.busy)) {
    think.hidden = true;
    if (thinkTimer) { clearInterval(thinkTimer); thinkTimer = null; }
    return;
  }
  if (!think.childNodes.length) {
    think.append(
      el("span", {class:"th-dots"}, el("span"), el("span"), el("span")),
      el("span", {class:"th-text"}, T["tui.chat.busy"] || "Thinking\u2026"),
      el("span", {class:"th-elapsed"}, ""));
  }
  think.hidden = false;
  positionThinking();
  // Start (or keep) the per-second elapsed ticker.
  if (!thinkTimer) {
    thinkStart = Date.now();
    const tick = () => {
      const e = think.querySelector(".th-elapsed");
      if (e) e.textContent = fmtElapsed(Date.now() - thinkStart);
    };
    tick();
    thinkTimer = setInterval(tick, 1000);
  }
}

// ── Status line ──────────────────────────────
// Until when the restart button is armed (wall clock, ms). Kept out here on
// purpose: the status line is rebuilt from scratch on every state push, so
// anything held on the element itself would be wiped a moment after arming
let restartArmed = 0;

// The restart button beside the stop button: relaunch the tab being viewed.
// Sends the intent that Ctrl+B r stands for, so there is one restart in the app.
//
// THE PHONE'S ONLY ONE. At the window the pair in each pane's caption is where
// restarting lives now, and it has to be: a status bar has one button and a
// divided screen has several panes, so "the tab being viewed" could only ever
// reach whichever pane had focus — you could not restart the other half of a
// split without first going and standing in it. A phone has no panes; it shows
// one screen at a time, so there the ambiguity does not arise and the bar is
// the right place. A phone watching an SSH tab that dropped is exactly who
// needs this.
//
// Where it applies is the app's call, carried in the state (`restartable`): a
// session relaunches its command, a page reopens exactly as it was opened (back
// to its starting URL, with everything the page had built up gone). The board
// and the app's own screens have nothing to put back.
//
// A tab that has already exited (the SSH that dropped) holds nothing to lose, so
// it goes on the first press. A live one asks twice: this sits next to the
// emergency stop, and a stray tap must not take a running conversation — or a
// filled-in form — down with it. The arming lapses on its own.
function restartBtn() {
  if (!REMOTE || !(S && S.restartable)) { restartArmed = 0; return null; }
  const t = S.tabs.find(x => x.index === S.active);
  const armed = Date.now() < restartArmed;
  return el("span", {id:"restart", class: armed ? "armed" : "",
    title:T["tui.restart.title"] || "",
    onclick:() => {
      if (armed || (t && t.state === "EXIT")) { restartArmed = 0; send({kind:"restart"}); }
      // Redraw once the arming lapses, in case no state push arrives to do it
      else { restartArmed = Date.now() + 4000; setTimeout(drawStatus, 4100); }
      drawStatus();
    }},
    armed ? (T["tui.restart.arm"] || "SURE?") : (T["tui.restart"] || "RESTART"));
}

function drawStatus() {
  const s = document.getElementById("status");
  s.textContent = "";
  // Passing null to append renders it as the literal string "null".
  // el() filters that out internally, but this is a raw append, so filter it out here too
  [
    el("span", {class:"wslink", title:T["tui.menu.workspace"] || "WORKSPACE",
      onclick:() => send({kind:"openws"})}, S.workspace || ""),
    el("span", {class:"pill " + (S.auto_enabled ? "on" : "off")},
      "AUTO " + (S.auto_enabled ? "ON" : "OFF")),
    S.remote_on ? el("span", {class:"pill on"}, "REMOTE") : null,
    // A phone is connected right now. Only shown at the window (a phone must not
    // be able to disconnect itself). Clicking it ends every remote session — the
    // phone's screen goes dark at once and its touches stop reaching anything —
    // and reclaims the window's own terminal width (a forced resize report), so
    // the name matches the deed. A fixed token says so: the cut is just as real,
    // but that phone can open the link again, and the button must not pretend
    // otherwise.
    (!REMOTE && S.remote_conn) ? el("span",
      {class:"pill live", id:"remotecut",
       title:(S.remote_sticky
         ? (T["tui.remote.cut.title.sticky"] || "A phone is connected — click to disconnect (the fixed token stays, so that phone can reconnect from the link)")
         : (T["tui.remote.cut.title"] || "A phone is connected — click to disconnect")),
       onclick:() => { send({kind:"remotecut"}); lastRC = ""; report(); }},
      T["tui.remote.live"] || "REMOTE ✕") : null,
    el("span", {class:"grow"}),
    el("span", {class:"build"}, BUILD),
    restartBtn(),
    el("span", {id:"stop", onclick:() => send({kind:"stop"})},
      T["tui.stop"] || "STOP"),
  ].forEach(x => { if (x) s.append(x); });
}

// ── Receiving surface ──────────────────────────────────
// The overlay screen. Closes on Esc or a click anywhere
function drawVeil() {
  const v = document.getElementById("veil");
  const shown = S.help_open || S.ws_open || !!S.qr;
  v.hidden = !shown;
  if (!shown) return;
  v.textContent = "";
  const box = el("div", {class:"box"});
  if (S.ws_open) {
    box.append(el("h3", {}, T["tui.workspace"] || "WORKSPACE"));
    S.workspaces.forEach((w, i) => {
      box.append(el("div", {class:"pick" + (i === S.ws_index ? " sel" : ""),
        onclick:() => send({kind:"menu", key:String(i + 1)})},
        (i + 1) + ".  " + w));
    });
  } else if (S.qr) {
    box.append(el("h3", {}, T["tui.menu.phone"] || "PHONE"));
    // The QR image arrives riding on the state (S.qr_svg). Making it a
    // separate image request meant it only rendered on one side of the
    // window/phone pair sharing this same page — a broken link on the other
    const qr = el("div", {class:"qr"});
    qr.innerHTML = S.qr_svg || "";
    // Show only the address, never the token. The token is a full-machine
    // credential now, so printing it as plain text under the QR would put it a
    // screenshot / OCR / shoulder-glance away from leaking. The QR image itself
    // still carries the token for scanning, which is the intended pairing path.
    box.append(qr, el("div", {class:"url"}, String(S.qr).split("?")[0]));
  } else {
    box.append(el("h3", {}, T["tui.help.title"] || "HELP"));
    // The keys come from the app, not from the translations: they are whatever
    // this person actually has, including anything they moved. Only the words
    // beside them are translated
    const wide = (S.help_rows || []).reduce((n, r) => Math.max(n, r[0].length), 0);
    for (const [press, what] of (S.help_rows || [])) {
      box.append(el("div", {class:"row"},
        el("b", {style:"display:inline-block;min-width:" + (wide + 2) + "ch"}, press),
        T[what] || what));
    }
    // The mouse has no keys to move, so those lines stay as they are written
    box.append(el("div", {class:"row"}, T["tui.help.mouse"]));
    for (const k of ["mouse.wheel", "mouse.drag", "mouse.right",
                     "mouse.tab", "mouse.divider"]) {
      box.append(el("div", {class:"row"}, T["tui.help." + k]));
    }
    box.append(el("div", {class:"row dim"}, T["tui.help.close"]));
  }
  v.onclick = () => send({kind:"key", named:"esc"});
  v.append(box);
}

// Prompt for a password. Never shown on the phone.
// There's no use case for it there, and since it's the same page being
// served, showing it would also expose it to anyone who opened the public settings
window.__password = function (title, note) {
  if (REMOTE) { send({kind:"password"}); return; }
  const v = document.getElementById("veil");
  v.hidden = false;
  v.textContent = "";
  const box = el("div", {class:"box"});
  const inp = el("input", {type:"password", autocomplete:"off"});
  inp.style.cssText = "font:inherit;background:var(--bg);color:var(--text);" +
    "border:1px solid var(--line);border-radius:6px;padding:8px 10px;width:320px";
  const done = t => { v.hidden = true; v.onmousedown = null; send({kind:"password", text:t}); };
  inp.onkeydown = e => {
    if (e.key === "Enter") { e.preventDefault(); done(inp.value); }
    if (e.key === "Escape") { e.preventDefault(); done(null); }
  };
  // Append the note only when there is one. Passing null to Element.append()
  // would stringify it and render the literal text "null".
  box.append(el("h3", {}, title));
  if (note) box.append(el("div", {class:"row"}, note));
  box.append(inp);
  // On the press, not the click -- see the vault and the palette below, and the
  // settings form's modal. A click is attributed to the ancestor shared by the
  // press and the release, so a selection dragged out of the box and released
  // over the backdrop reads as a click on the backdrop and takes the box away
  v.onmousedown = e => { if (e.target === v) done(null); };
  v.append(box);
  inp.focus();
};

// The top bar. Where a click goes is decided by Rust (only one bar is ever
// shown at a time, so this side never needs to say which page it's for)
const goTo = () => {
  const inp = document.querySelector("#nav input");
  if (inp && inp.value.trim()) send({kind:"go", what:"to", url:inp.value});
};
function drawNav() {
  const n = document.getElementById("nav");
  const want = S && S.nav;
  n.hidden = !want;
  n.classList.toggle("loading", !!(want && want.loading));   // in-flight loading band
  if (!want) { n.textContent = ""; layout(); return; }
  // Rebuilding while the user is mid-typing would erase what's typed so far, one character at a time
  const inp = n.querySelector("input");
  const typing = inp && document.activeElement === inp;
  if (!typing) {
    n.textContent = "";
    const btn = (mark, word, what, on) => {
      const b = el("button", {title:T[word]}, mark);
      b.disabled = !on;
      b.onclick = () => send({kind:"go", what:what});
      return b;
    };
    if (want.back) n.append(btn("←", "tui.nav.back", "back", want.can_back));
    if (want.forward) n.append(btn("→", "tui.nav.forward", "forward", want.can_forward));
    if (want.reload) {
      // The same act, twice, the way the pane header shows restarting: ⟳
      // carries on with what is already held, ⟲ goes back to nothing. It is
      // the same pair of ideas — keep the conversation / start a new one,
      // keep the cache / fetch it all again — so it is the same pair of
      // marks, and neither of them is hidden behind a key nobody presses.
      // Wrap the icon in a span so only it spins while loading
      const rb = el("button", {title:T["tui.nav.reload"]},
        el("span", {class:"ico"}, "⟳"));
      // Shift still works on the plain one: it is what a hand trained on
      // browsers will try, and it costs nothing to answer
      rb.onclick = (e) =>
        send({kind:"go", what:(e.shiftKey || e.ctrlKey || e.metaKey) ? "hardreload" : "reload"});
      if (want.loading) rb.classList.add("spin");
      n.append(rb);
    }
    // Its own switch, so a bar can show one, the other, or both
    if (want.reload_hard) {
      n.append(el("button", {title:T["tui.nav.reload_hard"],
          onclick:() => send({kind:"go", what:"hardreload"})},
        el("span", {class:"ico"}, "⟲")));
    }
    if (want.edit) {
      const box = el("input", {type:"text", spellcheck:"false",
        title:T["tui.nav.url.ph"], placeholder:T["tui.nav.url.ph"], value:want.at || ""});
      box.onkeydown = e => {
        if (e.key === "Enter") { e.preventDefault(); goTo(); }
        // Keystrokes here never flow to the terminal — this is where the destination URL is typed
        e.stopPropagation();
      };
      box.onfocus = () => box.select();
      n.append(box);
    }
  } else if (want.edit) {
    // Only fix up the enabled/disabled state of the buttons the user isn't currently typing into
    const bs = n.querySelectorAll("button");
    let i = 0;
    if (want.back && bs[i]) bs[i++].disabled = !want.can_back;
    if (want.forward && bs[i]) bs[i++].disabled = !want.can_forward;
  }
  layout();
}

// Push where the page sits down by the bar's height.
// The screen-relay canvas must be pushed down by the same amount, or the
// top edge of the browser view (often where a login form sits) ends up
// hidden behind the bar. Cursor coordinates are measured from the
// canvas's position, so they follow along automatically
function layout() {
  const n = document.getElementById("nav");
  // Reserved out of the focused pane's rectangle rather than pushed onto each
  // layer by hand: with panes, "the top of the screen" is no longer the top of
  // the window, and two layers being told different tops is how they drift
  document.getElementById("main").style.setProperty("--navh", n.hidden ? "0px" : "36px");
  report();
}

window.__state = function (json) {
  if (holding) { queued = json; return; }
  // The board is live now — take down the startup splash.
  const _sp = document.getElementById("splash");
  if (_sp && !_sp.hidden) _sp.hidden = true;
  S = JSON.parse(json);
  // A page older than the app it's talking to keeps rendering yesterday's
  // UI — a phone leaves the board open across app updates, and every "the
  // button is still the old one" report traces back to that. The state
  // carries the app's build stamp; on mismatch, one reload heals it (the
  // token survives in sessionStorage, so the page comes back signed in)
  if (S && S.build && BUILD && S.build !== BUILD && !window.__reloading) {
    window.__reloading = true;
    location.reload();
    return;
  }
  // The composer is the workbench: it shows by default wherever there is
  // something to type into (opened WITHOUT focus, so a phone's soft keyboard
  // stays down until the person taps the field). Only their own ✕ keeps it
  // collapsed — and then the ✏️ pen stays visible as the way back in.
  // A browser tab is the window's own case, handled by syncBrowserDock.
  if (!covering() && !castClosed() && !castMode
      && !(castDock && castDock.style.display === "flex")
      && (REMOTE ? onTermPty() : !onBrowserTab())) {
    openTermBar();
    if (castInput) castInput.blur();
  }
  // Where we are is half of what decides the pen, and it changes without the
  // bar being touched — so it is settled from the state, every update, on both
  // surfaces (see syncPen)
  syncPen();
  drawTabs();
  drawStatus();
  drawNav();
  const board = document.getElementById("board");
  const screen = document.getElementById("screen");
  // While viewing a browser tab, the embedded page covers the same spot.
  // Leaving the terminal contents in place would flash the previous tab's
  // output for a frame at the moment of switching
  const web = S.tabs.some(t => t.index === S.active && t.kind === "browser");
  // INDEX covers the window; the panes are still there underneath and come
  // back the moment a running thing is picked. Nothing of the layout is drawn
  // while it is up, or the caption of a pane would show through the board
  // Two screens cover the window: INDEX and the settings form. Neither is a
  // pane -- one is a view OF the running things and the other is about the app
  // itself -- so while either is up the layout waits underneath, whole
  const cover = !!S.board || !!S.settings_open;
  board.hidden = !S.board;
  document.getElementById("panes").hidden = cover;
  // Nothing to draw for a pane with nothing in it -- it says so itself
  screen.hidden = cover || S.active === 0 || web;
  // While viewing a browser tab, the phone shows the screen relay (canvas).
  // The window (PC) still layers the real page as before, so it never uses the relay
  const cast = document.getElementById("cast");
  cast.hidden = !(web && REMOTE);
  if (web && REMOTE) castStart(); else castStop();
  // Window only: over a browser tab, reuse the sub-input bar (composer) — actions
  // only, no target — and reserve room so the native browser doesn't hide it.
  syncBrowserDock();
  // Page buttons: only on a phone, only over a screen a person reads page by
  // page. A model pane's conversation arrives whole, so it has no pages.
  const pager = document.getElementById("pageui");
  if (pager) {
    const showPager = REMOTE && !screen.hidden && !web && !onModelTab();
    pager.classList.toggle("on", showPager);
    if (!showPager) pgReset();
    // 📖 rides with the pager because it answers the same need — reading what
    // was said — and offers the better half of the answer wherever there is a
    // record to read. Where there is none it is not shown at all: a button that
    // does nothing on some tabs teaches people not to trust it
    const openRead = document.getElementById("readOpen");
    const here = activeTab();
    if (openRead) {
      openRead.style.display = (showPager && here && here.readable) ? "flex" : "none";
      openRead.title = T["tui.read.open"] || "";
    }
  }
  rdOnState();
  // ...and it goes away when there is not. INDEX and the settings form cover
  // the panes, so a Send there would be addressed to a pane that is not in
  // front and would reach nobody; the phone additionally shuts it over
  // anything that is not a pane it can type into (a stray relay dock is handled
  // separately, by castStop → exitCast). Shut by us, not by the person, so the
  // ✕ preference is left alone and the bar returns on its own above.
  if (!castMode && castDock && castDock.style.display === "flex") {
    if (covering() || (REMOTE && (screen.hidden || web || !onTermPty()))) closeBar();
  }
  // 📼 recording is "record what I do on THIS page": leaving the tab ends it,
  // so the radio never claims a recording that moved out from under it. The
  // server side disarms every page on off, so this can't miss the right one.
  if (S.active !== lastCastActive && luaMode === "rec") {
    luaMode = "run";
    send({kind:"record", on:false});
  }
  // The panel area follows the active tab: which panels exist (a browser tab has
  // no 🎯 target panel, so the switcher itself comes and goes) and the target
  // panel's operator gating both depend on it. If the active tab changed while
  // the dock is open, rebuild whatever is showing so none of it goes stale.
  if (castDock && castDock.style.display === "flex" && S.active !== lastCastActive) {
    renderPanel();
  }
  lastCastActive = S.active;
  if (S.board) drawBoard();
  drawTopicBar();
  drawThinking();
  drawVeil();
  renderVault();
  drawBranch();
  drawBrowse();
  // While scrolled back through history, say so — clicking jumps back to the present
  const b = document.getElementById("back");
  const away = !screen.hidden && S.scrolled > 0;
  b.hidden = !away;
  if (away) {
    b.textContent = (T["tui.scrolled"] || "").replace("{lines}", S.scrolled);
    b.onclick = () => send({kind:"scroll", by: -1000000});
  }
  // The app's own message arrives as state, and is shown as the same toast
  // everything else on this screen uses. Compared by value, so a state push
  // that merely repaints (a new frame, a tab switch, a phone reconnecting)
  // can't resurrect a message that has already faded or been dismissed.
  if (S.flash !== lastFlash) {
    lastFlash = S.flash;
    if (S.flash) toast(S.flash); else hideToast();
  }
  paintPaneHeads();
};

// How the content area is divided. Panes arrive as fractions of it, so the
// page can lay them out with percentages and let the browser do the arithmetic
// at whatever the window's real size happens to be.
//
// Only the focused pane gets the full renderer (the terminal with its cursor,
// the dashboard, a placed browser and its bar). The rest get a read-only view
// of their terminal, which is all a pane you are not typing into can show.
let PANES = null;
window.__panes = function (json) {
  const P = JSON.parse(json);
  PANES = P;
  const host = document.getElementById("panes");
  const seen = new Set();
  for (const p of P.panes) {
    seen.add(String(p.id));
    let el = host.querySelector('.pane[data-pid="' + p.id + '"]');
    if (!el) {
      el = document.createElement("div");
      el.className = "pane";
      el.dataset.pid = p.id;
      el.innerHTML = '<div class="phead"><span class="dot"></span>' +
        '<span class="nm"></span>' +
        // ▥ lines running down = a division down the middle; ▤ lines running
        // across = a division across. A matched pair, so the two read as one
        // choice with two directions rather than as two unrelated icons
        '<span class="sp sr" title="' + (T["tui.pane.split_right"] || "") + '">&#9637;</span>' +
        '<span class="sp sd" title="' + (T["tui.pane.split_down"] || "") + '">&#9636;</span>' +
        // Same act, two directions in time: ⟳ carries the conversation on,
        // ⟲ goes back to the start of one. A matched pair, the way ▥ and ▤ are
        '<span class="rs rk" title="' + (T["tui.pane.restart_keep"] || "") + '">&#10227;</span>' +
        '<span class="rs rf" title="' + (T["tui.pane.restart_fresh"] || "") + '">&#10226;</span>' +
        '<span class="cl">&#10005;</span></div>' +
        '<div class="pbody"><pre class="pscreen notranslate" translate="no"></pre>' +
        '<div class="pnew"></div></div>';
      // Clicking anywhere in a pane you are not in moves you there. The close
      // control is the one thing inside it that means something else.
      el.onmousedown = (e) => {
        if (p.id === (PANES && PANES.focus)) return;
        send({kind:"focuspane", id:p.id});
      };
      el.querySelector(".cl").onclick = (e) => {
        e.stopPropagation();
        send({kind:"closepane", id:p.id});
      };
      // An empty pane invites the tab that will fill it. The settings screen
      // opens on its add-a-tab form, and saving puts the new tab in THIS pane
      // -- the one that was pressed, not whichever had focus by the time the
      // form was done with
      el.querySelector(".pnew").textContent = T["tui.pane.add"] || "+ Add tab";
      el.querySelector(".pnew").onclick = (e) => {
        e.stopPropagation();
        send({kind:"addtab", pane:p.id});
      };
      // Same two divisions the keyboard makes, on the pane you pressed them on
      for (const [cls, down] of [[".sr", false], [".sd", true]]) {
        el.querySelector(cls).onclick = (e) => {
          e.stopPropagation();
          send({kind:"splitpane", id:p.id, down});
        };
      }
      // The same two keys, Ctrl+B r and Ctrl+B R, on the pane you pressed them
      // on. A pane whose thing has already exited (the SSH that dropped) holds
      // nothing to lose, so it goes on the first press; a live one asks twice,
      // because a stray tap must not take a running conversation down with it
      for (const [cls, keep] of [[".rk", true], [".rf", false]]) {
        el.querySelector(cls).onclick = (e) => {
          e.stopPropagation();
          const t = paneTab(p);
          if (armedPane === cls + p.id || (t && t.state === "EXIT")) {
            armedPane = null;
            send({kind:"restartpane", id:p.id, keep});
          } else {
            // One thing armed at a time: arming the other of the pair, or the
            // same one on another pane, must not leave a second live trigger
            armedPane = cls + p.id;
            setTimeout(() => {
              if (armedPane === cls + p.id) { armedPane = null; paintPaneHeads(); }
            }, 4100);
          }
          paintPaneHeads();
        };
      }
      host.append(el);
    }
    el.style.left = (p.x * 100) + "%";
    el.style.top = (p.y * 100) + "%";
    el.style.width = (p.w * 100) + "%";
    el.style.height = (p.h * 100) + "%";
    el.classList.toggle("focused", !!p.focused);
    // Captioned even when it is the only one. The caption is where ▥ and ▤
    // live, and without it the first division could only be asked for with the
    // keyboard -- a whole feature with no way in for a hand on the mouse. The
    // tab bar does say what you are looking at, but it cannot divide it
    el.classList.add("headed");
    // ✕ closes a pane, and the last one cannot be closed. A control that
    // refuses is worse than one that is not offered
    el.querySelector(".cl").hidden = !!P.single;
    if (p.focused) el.querySelector(".pscreen").textContent = "";
  }
  for (const el of [...host.querySelectorAll(".pane")]) {
    if (!seen.has(el.dataset.pid)) el.remove();
  }
  paintDividers(P.dividers || []);
  paintPaneHeads();
  measureFocused();
  lastRC = "";
  report();
  if (lastCur) window.__cursor(lastCur[0], lastCur[1], lastCur[2]);
};

// Which restart is one press away from firing, as "<class><pane id>". At most
// one at a time, and never held on the element: the captions are repainted on
// every state push, and an arming kept there would be wiped a moment after it
// was asked for
let armedPane = null;

// The tab a pane is showing, if it is showing one.
function paneTab(p) {
  return S && S.tabs ? S.tabs.find(x => x.index === p.surface) : null;
}

// What each pane is captioned with. Read from the same state the tab bar uses
// rather than sent along with the tree: one copy cannot go stale against the other.
function paintPaneHeads() {
  if (!PANES) return;
  for (const p of PANES.panes) {
    const el = document.querySelector('#panes .pane[data-pid="' + p.id + '"]');
    if (!el) continue;
    const t = paneTab(p);
    el.querySelector(".nm").textContent = t ? t.name : "";
    el.querySelector(".dot").className = "dot " + (t ? t.state : "");
    // Offered only where it would do something. The app's own screens (the
    // settings form, the result view) have nothing behind them to put back,
    // and a control that refuses is worse than one that is not there
    for (const cls of [".rk", ".rf"]) {
      const b = el.querySelector(cls);
      b.hidden = !(t && t.restartable);
      b.classList.toggle("armed", armedPane === cls + p.id);
    }
    // Empty means there is nothing to show here, which is the same question
    // this line already answers. Asking the pane tree instead -- "is the
    // surface number zero?" -- was asking a copy that can be a frame behind,
    // and a pane holding a number that no longer names anything is just as
    // empty as one holding none
    el.classList.toggle("empty", !t);
  }
}

// The grab handles between panes.
//
// Rust names each divider by its position in its own list and hands over the
// area it divides; the page only turns that into pixels and hands a new ratio
// back. Which split a boundary belongs to is never guessed from the way the
// screen looks — two panes can look adjacent and belong to splits several
// levels apart, and guessing would move a divider elsewhere on the screen.
const DIV_GRAB = 9;   // px: what the pointer can catch, not what the eye sees
function paintDividers(list) {
  const host = document.getElementById("panes");
  const live = new Set();
  for (const d of list) {
    live.add(String(d.i));
    let el = host.querySelector('.pdiv[data-di="' + d.i + '"]');
    if (!el) {
      el = document.createElement("div");
      el.dataset.di = d.i;
      el.title = T["tui.pane.divider"] || "";
      el.onmousedown = (e) => startDividerDrag(e, el);
      // Back to even halves. The keyboard has no word for this, and after a
      // few drags "put it back" is the thing people want most
      el.ondblclick = (e) => {
        e.preventDefault();
        send({kind:"paneratio", divider: Number(el.dataset.di), ratio: 0.5});
      };
      host.append(el);
    }
    el.className = "pdiv " + (d.down ? "h" : "v");
    // The handle straddles the line, so half of it hangs over each side
    if (d.down) {
      const at = (d.y + d.h * d.ratio) * 100;
      el.style.left = (d.x * 100) + "%";
      el.style.width = (d.w * 100) + "%";
      el.style.top = "calc(" + at + "% - " + (DIV_GRAB / 2) + "px)";
      el.style.height = DIV_GRAB + "px";
    } else {
      const at = (d.x + d.w * d.ratio) * 100;
      el.style.top = (d.y * 100) + "%";
      el.style.height = (d.h * 100) + "%";
      el.style.left = "calc(" + at + "% - " + (DIV_GRAB / 2) + "px)";
      el.style.width = DIV_GRAB + "px";
    }
    el._span = d;
  }
  for (const el of [...host.querySelectorAll(".pdiv")]) {
    if (!live.has(el.dataset.di)) el.remove();
  }
}

// Dragging one. The ratio is recomputed from where the pointer is inside the
// area that divider owns, so the divider follows the pointer exactly instead
// of drifting by however much the first movement missed the line by.
function startDividerDrag(e, el) {
  e.preventDefault();
  const host = document.getElementById("panes");
  const span = el._span;
  if (!span) return;
  el.classList.add("dragging");
  document.body.classList.add("dragdiv");
  const move = (ev) => {
    const box = host.getBoundingClientRect();
    const frac = span.down
      ? ((ev.clientY - box.top) / box.height - span.y) / span.h
      : ((ev.clientX - box.left) / box.width - span.x) / span.w;
    const ratio = Math.max(0.1, Math.min(0.9, frac));
    // Draw it where the pointer is right away. The tree will come back with
    // the same number a frame later; waiting for that first would make the
    // divider lag behind the hand
    if (span.down) {
      el.style.top = "calc(" + ((span.y + span.h * ratio) * 100) + "% - " + (DIV_GRAB / 2) + "px)";
    } else {
      el.style.left = "calc(" + ((span.x + span.w * ratio) * 100) + "% - " + (DIV_GRAB / 2) + "px)";
    }
    send({kind:"paneratio", divider: Number(el.dataset.di), ratio});
  };
  const up = () => {
    el.classList.remove("dragging");
    document.body.classList.remove("dragdiv");
    window.removeEventListener("mousemove", move);
    window.removeEventListener("mouseup", up);
  };
  window.addEventListener("mousemove", move);
  window.addEventListener("mouseup", up);
}

// ── The tab bar's width ──────────────────────────────────
// Dragged by its edge, like the dividers between panes. The bounds are the
// app's, handed in rather than written twice: the window clamps whatever
// arrives, and a page that guessed different numbers would show one width
// while the next start opened another.
const TABW_MIN = {{TAB_W_MIN}}, TABW_MAX = {{TAB_W_MAX}}, TABW_DEF = {{TAB_W_DEF}};
// The width to come back to when the bar is brought out again. A bar that is
// put away has no width to remember, so this holds the last real one.
//
// Only ever taken from a width the bar came to REST at, never from one it
// passed through. A drag that shuts the bar sweeps through every width down to
// the minimum on the way, and remembering those meant dragging it shut and
// pressing the key gave you a sliver instead of the bar you had
let lastTabW = TABW_DEF;

function tabWidth() {
  const v = parseFloat(getComputedStyle(document.documentElement)
    .getPropertyValue("--tabw"));
  return isFinite(v) ? Math.round(v) : TABW_DEF;
}
// Set it, draw it, and tell the app -- which writes it down so the next start
// opens the same way. The terminal is re-measured because it just got wider or
// narrower, and an AI handed the wrong column count wraps its screen wrongly.
function setTabWidth(px) {
  const w = px <= 0 ? 0 : Math.max(TABW_MIN, Math.min(TABW_MAX, Math.round(px)));
  document.documentElement.style.setProperty("--tabw", w + "px");
  send({kind:"tabwidth", px: w});
  scheduleReport();
}
// The bar has come to rest at whatever it is now: that is a width worth
// coming back to
function settleTabWidth() {
  const w = tabWidth();
  if (w > 0) lastTabW = w;
}
// Put the bar away, or bring it back the width it was. The same one number
// says which, so there is no second flag to fall out of step with it
window.__toggleTabBar = function () {
  if (tabWidth() > 0) { settleTabWidth(); setTabWidth(0); }
  else setTabWidth(lastTabW);
};

(function () {
  const grip = document.getElementById("tabgrip");
  if (!grip) return;
  grip.title = T["tui.tabbar.grip"] || "";
  grip.onmousedown = (e) => {
    e.preventDefault();
    grip.classList.add("dragging");
    document.body.classList.add("dragdiv");
    const left = document.getElementById("app").getBoundingClientRect().left;
    const move = (ev) => {
      const want = ev.clientX - left;
      // Dragged nearly shut means shut. Without this the bar would stick at
      // its own minimum and the one thing the drag looks like it should do --
      // get it out of the way -- would be the one thing it could not do
      setTabWidth(want < TABW_MIN / 2 ? 0 : want);
    };
    const up = () => {
      grip.classList.remove("dragging");
      document.body.classList.remove("dragdiv");
      settleTabWidth();
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
  };
  // Back to the width it ships with, the way double-clicking a pane divider
  // puts that back to even halves
  grip.ondblclick = (e) => { e.preventDefault(); setTabWidth(TABW_DEF); settleTabWidth(); };
  settleTabWidth();
})();

// One unfocused pane's terminal contents.
window.__panescreen = function (id, html) {
  const el = document.querySelector('#panes .pane[data-pid="' + id + '"] .pscreen');
  if (el) el.innerHTML = rowsHtml(html);
};

// The terminal's own contents, as one element per row.
//
// Rows are elements rather than newlines in one blob so that the usual change
// -- an AI's spinner turning over, one line of output arriving -- can be
// written into that one row (see __rows). Replacing the whole grid instead
// makes the browser rebuild every element on screen, and the person typing in
// the composer waits for that layout before their own keystroke can land.
function rowsHtml(html) {
  const rows = html.split("\n");
  let out = "";
  for (let i = 0; i < rows.length; i++) out += '<div class="r">' + rows[i] + '</div>';
  return out;
}
// The rows that moved since the last frame, as [row number, html]. The window
// sends these whenever it can; a full __screen still arrives when the shape of
// the screen changed (a resize, a switched tab) or when most of it did anyway
window.__rows = function (list) {
  const kids = document.getElementById("screen").children;
  for (let i = 0; i < list.length; i++) {
    const el = kids[list[i][0]];
    if (el) el.innerHTML = list[i][1];
  }
};
// The whole grid. This is the one place a grid of cells is correct, so accept it as-is
window.__screen = function (html) {
  const s = document.getElementById("screen");
  // A phone shows the window's own terminal size (it can't resize it from afar),
  // which is often taller than the phone's viewport. The bottom of the frame —
  // a TUI's input line and its newest output — then falls below the fold, and
  // the pager only moves the app's own scrollback, never THIS viewport. So on a
  // phone keep the viewport pinned to the live bottom, unless the reader has
  // scrolled up to look back within the current frame (then hold their place —
  // innerHTML would otherwise snap it to the top on every update).
  const unit = cellH || 16;
  const nearBottom = (s.scrollHeight - s.clientHeight - s.scrollTop) <= unit * 1.5;
  const prevTop = s.scrollTop;
  s.innerHTML = rowsHtml(html);
  if (REMOTE) s.scrollTop = nearBottom ? s.scrollHeight : prevTop;
};

// ── Input handling starts here ────────────────────────────
// Only the terminal's contents are a grid of cells, so measuring and overlaying only ever happens here
const scr = document.getElementById("screen");
const kbd = document.getElementById("kbd");
const cur = document.getElementById("cur");
const probe = document.getElementById("probe");
const tprobe = document.getElementById("tprobe");
let cellW = 0, cellH = 0, curX = 8, curY = 8, composing = false;
let gRows = 24;   // last measured terminal height in rows (used by the remote pager)
// The last-reported cursor position. Re-placed here whenever measurements are redone
let lastCur = null;

// Setting the shorthand `font` property falls back to an empty string for
// combinations the browser can't resolve. Assigning an empty string does
// nothing visibly, but silently measures with a different font — so copy each property individually
function copyFont(el2) {
  const c = getComputedStyle(scr);
  el2.style.fontFamily = c.fontFamily;
  el2.style.fontSize = c.fontSize;
  el2.style.fontWeight = c.fontWeight;
  el2.style.letterSpacing = c.letterSpacing;
}
function measure() {
  copyFont(probe);
  const r = probe.getBoundingClientRect();
  cellW = r.width / 10;
  cellH = parseFloat(getComputedStyle(scr).lineHeight) || r.height;
  // Both the content columns and the cursor are placed using this one number.
  //
  // Previously the content used ch (the font's own advance width for "0")
  // while the cursor used the measured value — two separate numbers.
  // Any tiny difference between them compounded column by column, so the
  // cursor drifted further right the more you typed. It's not about which
  // number is "correct" — what matters is placing both from the same number
  scr.style.setProperty("--cw", cellW + "px");
}

// The font arrives later, asynchronously. Measuring before it loads locks
// in column widths based on the fallback font's metrics.
// Once it arrives, re-measure and re-place everything
if (document.fonts && document.fonts.ready) {
  document.fonts.ready.then(() => {
    cellW = 0;
    measure();
    lastRC = "";
    report();
    if (lastCur) window.__cursor(lastCur[0], lastCur[1], lastCur[2]);
  });
}

// Report how many rows and columns fit in the window.
// The other side trusts this number for line-wrapping, so a mismatch
// means it keeps writing past the edge of the screen
let lastRC = "";
// The last measurements taken while the panes were actually on screen. A screen
// that covers them -- INDEX, the settings form -- must not be allowed to speak
// for their size; see report().
let lastPanes = null, lastFit = null;
// The focused pane's rectangle, in the pixels every layer above it draws with:
// the placed browser, the composer, the pen.
//
// Measured wherever the geometry can move, not only when the pane tree
// changes. These are pixels and the panes are laid out in percentages, so a
// window resize -- or a drag of the tab bar's edge -- reflows the panes and
// leaves these describing the window as it used to be. A browser placed by
// them then landed in the old rectangle, and focusing another pane redrew the
// tree and quietly put it right, which made it look like a focus bug.
function measureFocused() {
  const main = document.getElementById("main");
  if (!main) return;
  const f = document.querySelector("#panes .pane.focused .pbody");
  const m = main.getBoundingClientRect();
  // A screen that covers the panes -- INDEX, the settings form -- leaves
  // nothing to measure: #panes is hidden, and a hidden element reports a
  // rectangle of zeros rather than no rectangle at all. Read as a pane, those
  // zeros describe one sitting off the top-left corner of the window, and
  // everything placed by these numbers went with it: on INDEX the toast was
  // drawn at left:-290px, bottom:998px in a 900px-tall window, so every menu
  // item that answers with a message looked like a button that does nothing.
  // With the panes covered, the thing being talked about IS the content area.
  const laid = f && f.getClientRects().length > 0;
  const b = laid ? f.getBoundingClientRect() : m;
  main.style.setProperty("--fx", (b.left - m.left) + "px");
  main.style.setProperty("--fy", (b.top - m.top) + "px");
  main.style.setProperty("--fr", (m.right - b.right) + "px");
  main.style.setProperty("--fb", (m.bottom - b.bottom) + "px");
}

function report() {
  measure();
  // Before anything is read off #page: it is placed by the numbers above, so
  // reading it first would report the rectangle the window used to have
  measureFocused();
  if (!cellW || !cellH) return;
  const pad = (parseFloat(getComputedStyle(scr).paddingLeft) || 0) * 2;
  const fit = (b) => ({
    cols: Math.max(20, Math.floor((b.width - pad) / cellW)),
    rows: Math.max(5, Math.floor((b.height - pad) / cellH)),
  });
  const main = document.getElementById("main");
  // Every pane is measured, not just the one in front. A terminal that isn't
  // focused is still running, and a wrong column count there is just as wrong —
  // it only looks harmless because nobody is typing into it at that moment.
  const boxes = [...document.querySelectorAll("#panes .pane")];
  // Rows/columns come from the pane's own body; the browser view's placement
  // comes from #page. Deriving both from a single rectangle would shrink the
  // terminal just because the top bar appeared, or re-wrap the AI's screen just
  // because a browser tab was switched to
  const area = document.getElementById("page").getBoundingClientRect();
  // INDEX and the settings form cover the panes, and a covered element reports
  // a rectangle of zeros rather than no rectangle at all. Measured as a pane,
  // those zeros are the smallest terminal `fit` will name -- 20x5 -- so every
  // running AI was told its window had shrunk to that, reflowed its whole
  // interface to suit, and was told to grow back the moment the cover lifted.
  // What came back from that round trip was a broken frame: blank rows, and
  // the box the program had drawn cut off mid-line. It repaired only where the
  // next keystroke made the program draw again, which is why typing appeared
  // to fix it a piece at a time. A covered pane has not changed size -- it is
  // only not being shown -- so keep the last measurement taken while it was.
  const laid = boxes.length > 0
    && boxes.every((el) => el.querySelector(".pbody").getClientRects().length > 0);
  if (laid) lastPanes = boxes.map((el) => {
    const b = el.querySelector(".pbody").getBoundingClientRect();
    const d = fit(b);
    // Where a browser placed in this pane sits. Even if the shell's CSS
    // changes, only the page itself knows this — never let Rust guess the
    // coordinates. In the focused pane that rectangle is #page, which already
    // has the browser bar's height taken out of it.
    const r = el.classList.contains("focused") ? area : b;
    // A placed browser is a native layer ON TOP of this page, so wherever it
    // sits, the page underneath stops receiving the pointer. Left flush against
    // the divider, it would swallow the half of the grab handle that overhangs
    // it — the divider between a terminal and the browser it is driving could
    // then only be caught from the terminal side. Hold the browser back by the
    // handle's overhang so the whole width of it stays reachable. Undivided,
    // there is no divider and nothing to hold back from
    const in_ = (PANES && !PANES.single) ? Math.ceil(DIV_GRAB / 2) : 0;
    return {id: +el.dataset.pid, rows: d.rows, cols: d.cols,
      rect: [Math.round(r.left) + in_, Math.round(r.top) + in_,
             Math.max(1, Math.round(r.width) - in_ * 2),
             Math.max(1, Math.round(r.height) - in_ * 2)]};
  });
  // The focused pane's numbers are the ones the rest of the app still speaks in
  if (laid) {
    const box = boxes.find((el) => el.classList.contains("focused"));
    lastFit = fit(box ? box.querySelector(".pbody").getBoundingClientRect()
                      : main.getBoundingClientRect());
  }
  const panes = lastPanes || [];
  // Before any pane has ever been laid out -- a cold start onto INDEX -- there
  // is nothing to remember yet, and the content area is the honest answer
  const f = lastFit || fit(main.getBoundingClientRect());
  gRows = f.rows;   // remembered so the remote pager knows one screenful's height
  const key = f.rows + "x" + f.cols + "@" +
    Math.round(main.getBoundingClientRect().width) + "," + Math.round(area.left) + "," +
    Math.round(area.top) + "," + Math.round(area.width) + "," + Math.round(area.height) +
    "|" + panes.map(p => p.id + ":" + p.rows + "x" + p.cols + ":" + p.rect.join(",")).join(";");
  if (key === lastRC) return;
  lastRC = key;
  // The whole content area as well as the focused pane's share of it. A
  // screen that covers the window -- the settings form -- is a page placed in
  // the window like any other and needs a rectangle; it is just not a pane's
  const whole = main.getBoundingClientRect();
  send({kind:"resize", rows:f.rows, cols:f.cols,
    area:[Math.round(area.left), Math.round(area.top),
          Math.round(area.width), Math.round(area.height)],
    full:[Math.round(whole.left), Math.round(whole.top),
          Math.round(whole.width), Math.round(whole.height)],
    panes:panes});
}
let rt = 0;
const scheduleReport = () => { clearTimeout(rt); rt = setTimeout(report, 80); };
window.addEventListener("resize", scheduleReport);
// A window 'resize' is not the only thing that changes the terminal's size,
// and relying on it alone leaves a stale, too-narrow `cols` frozen in — so the
// AI keeps wrapping its output at a fraction of the real width, with a wide
// black margin on the right. #main can resize with no window 'resize' at all:
// the startup splash being removed, the mobile layout settling in a frame late,
// the drawer opening/closing, switching back from a browser tab, or an
// orientation change some WebViews never report as a window resize. Observe the
// element itself so ANY change to its rendered size re-reports the true
// row/column count and the screen always fills the available width. Reporting
// the accurate width also keeps ASCII art aligned — it only breaks when `cols`
// disagrees with the space actually drawn.
if (window.ResizeObserver) {
  new ResizeObserver(scheduleReport).observe(document.getElementById("main"));
}

window.__cursor = function (row, col, shown) {
  lastCur = [row, col, shown];
  if (!cellW) measure();
  const pad = parseFloat(getComputedStyle(scr).paddingLeft) || 0;
  const padT = parseFloat(getComputedStyle(scr).paddingTop) || 0;
  // Both #cur and #kbd live inside #main. left/top are distances from
  // #main, so using viewport-wide coordinates would shift everything right by the tab bar's width
  const frame = document.getElementById("main").getBoundingClientRect();
  const box = scr.getBoundingClientRect();
  // The content is scrollable — the text moves along with it by exactly the same amount
  curX = (box.left - frame.left) + pad + col * cellW - scr.scrollLeft;
  curY = (box.top - frame.top) + padT + row * cellH - scr.scrollTop;
  kbd.style.left = curX + "px";
  kbd.style.top = curY + "px";
  kbd.style.height = cellH + "px";
  // Hide it whenever the terminal isn't in view. A leftover cursor sitting
  // over the dashboard or a browser tab would look like it means something there
  // On a model pane the caret lives in the composer, so hide the screen's.
  cur.hidden = !shown || composing || S === null || scr.hidden || onModelTab();
  if (!cur.hidden) {
    cur.style.left = curX + "px";
    cur.style.top = curY + "px";
    cur.style.width = cellW + "px";
    cur.style.height = cellH + "px";
  }
  positionThinking();
};

// Park the thinking indicator right at the cursor — where "generating" used to
// print — so it reads as part of the conversation, not a floating widget.
function positionThinking() {
  const think = document.getElementById("thinking");
  if (!think || think.hidden) return;
  think.style.left = curX + "px";
  think.style.top = curY + "px";
  think.style.height = cellH + "px";
}

// Never send while the IME is still composing — only send confirmed text.
// The width is measured by actually rendering it, not estimated from
// character count (mixing full-width and half-width characters always throws that off)
function widen(s) {
  copyFont(tprobe);
  tprobe.textContent = s || "";
  const need = tprobe.getBoundingClientRect().width + cellW * 2;
  // Width is also computed within #main. If it would overflow, shift left
  const room0 = document.getElementById("main").clientWidth;
  let left = curX;
  if (curX + need > room0 - 8) {
    left = Math.max(0, room0 - need - 8);
  }
  kbd.style.left = left + "px";
  const room = Math.max(cellW, room0 - left - 8);
  kbd.style.width = Math.max(need, room) + "px";
}
kbd.addEventListener("compositionstart", () => { composing = true; cur.hidden = true; widen(""); });
kbd.addEventListener("compositionupdate", e => widen(e.data));
kbd.addEventListener("compositionend", e => {
  composing = false;
  kbd.value = "";
  kbd.style.width = "1px";
  if (e.data) send({kind:"key", text:e.data});
});
kbd.addEventListener("input", e => {
  if (composing || e.isComposing) return;
  const v = kbd.value;
  kbd.value = "";
  if (v) send({kind:"key", text:v});
});

const NAMED = {
  Enter:"enter", Backspace:"bs", Tab:"tab", Escape:"esc", Delete:"del",
  ArrowUp:"up", ArrowDown:"down", ArrowRight:"right", ArrowLeft:"left",
  Home:"home", End:"end", PageUp:"pgup", PageDown:"pgdn",
  F1:"f1", F2:"f2", F3:"f3", F4:"f4", F5:"f5", F6:"f6",
  F7:"f7", F8:"f8", F9:"f9", F10:"f10", F11:"f11", F12:"f12",
};
kbd.addEventListener("keydown", e => {
  if (e.isComposing) return;
  const nm = NAMED[e.key];
  if (nm) { e.preventDefault(); send({kind:"key", named:nm}); return; }
  if (e.ctrlKey && e.key.length === 1) {
    e.preventDefault();
    send({kind:"key", ctrl:e.key.toLowerCase()});
  }
});

// Same convention as PuTTY: selecting text copies it immediately, right-click pastes.
// Except while typing in the URL bar — stealing focus there would block every keystroke
// The tab in view, once, so nothing has to re-derive it from S.active. Every
// "am I on a X tab" question below is asked of this.
const activeTab = () => (S && S.tabs) ? S.tabs.find(t => t && t.index === S.active) : null;
// Whether a screen is covering the panes. INDEX and the settings form are not
// panes -- one is a view OF the running things, the other is about the app
// itself -- so while either is up there is no pane in front, and anything that
// belongs to a pane (the composer, the pen) belongs nowhere.
const covering = () => !!(S && (S.board || S.settings_open));
// Whether a terminal tab (session) is currently in view. False for INDEX(0), browser tabs, and unknown state
const onTerminal = () => !!(S && !S.board && S.active !== 0 && S.tabs &&
  S.tabs.some(t => t.index === S.active && t.kind !== "browser"));
// Whether the pane in view is answered by a model over the API rather than by a
// program at a prompt. It still has a screen and a scrollback like any other —
// what differs is only where a Send goes (sendBar) and which panels make sense
// there (panelOptions).
const onModelTab = () => { const t = activeTab(); return !!(t && t.model); };
// A pane a person can type into: any session, model bridge included. Excludes
// INDEX(0) and browser relays only. A model pane used to be excluded too, back
// when it carried a composer of its own; it has none now, so leaving it out
// would leave it with no way in at all.
const onTermPty = () => onTerminal();
const focus = () => {
  const a = document.activeElement;
  if (a && a.closest && a.closest("#nav")) return;
  // Never steal focus while a text field is being used (the cast input bar, the
  // discussion-topic box, etc.). Otherwise every keystroke would be swallowed by
  // #kbd and fired as a board shortcut (e.g. typing "w" opens the workspace list).
  if (a && (a.tagName === "INPUT" || a.tagName === "TEXTAREA")) return;
  // A model pane has no command line to type at: its only input is the
  // composer, so the caret belongs there (like Claude, whose cursor always sits
  // in the prompt). If the person has collapsed the bar, the \u270f\ufe0f pen is the way
  // back and nothing is focused meanwhile.
  if (onModelTab()) {
    if (castInput && castDock && castDock.style.display === "flex") castInput.focus();
    return;
  }
  // On a phone, terminal typing never goes through the hidden #kbd (which would
  // pop the soft keyboard up over the screen). It goes through the sub-input bar,
  // opened on a tap — see the mouseup handler and openTermBar(). The window (PC)
  // keeps its previous behavior (#kbd is needed for its menu keys and inline caret).
  if (REMOTE) return;
  kbd.focus();
};
// Scroll back through history with the wheel.
//
// The terminal only ever shows a single screen's worth at a time — the
// rest is held by the other side. This just reports how far to rewind; the side holding the buffer decides what to show
scr.addEventListener("wheel", e => {
  if (!S || S.active === 0 || scr.hidden) return;
  e.preventDefault();
  // Ctrl+wheel is what a person already tries in a terminal, a browser and an
  // editor. Nothing else here uses it, so it costs no key and needs no telling
  if (e.ctrlKey) { zoom(e.deltaY < 0 ? 1 : -1); return; }
  if (!cellW || !cellH) measure();
  // Full-screen programs handle their own scrollback. Pass along which cell it's over, too
  const pad = parseFloat(getComputedStyle(scr).paddingLeft) || 0;
  const padT = parseFloat(getComputedStyle(scr).paddingTop) || 0;
  const box = scr.getBoundingClientRect();
  const col = Math.max(0, Math.floor((e.clientX - box.left - pad) / cellW));
  const row = Math.max(0, Math.floor((e.clientY - box.top - padT + scr.scrollTop) / cellH));
  // Scrolling up (deltaY < 0) means going further back. Count each notch as one step
  const n = Math.max(1, Math.round(Math.abs(e.deltaY) / 100));
  send({kind:"scroll", by: e.deltaY < 0 ? n : -n, row: row, col: col});
}, {passive:false});

// How big the terminal is drawn. The page changes it at once — pixels are the
// page's business — and tells the other side to remember it. Re-measuring is
// what turns a font change into a differently-shaped terminal: the cell grid is
// measured, and the rows and columns reported from it are what the program in
// the tab is told to draw at
function zoom(by) {
  const now = parseFloat(getComputedStyle(document.documentElement).getPropertyValue("--fs")) || 14;
  const px = Math.min(32, Math.max(8, Math.round(now + by)));
  if (px === now) return;
  document.documentElement.style.setProperty("--fs", px + "px");
  cellW = 0; cellH = 0;
  measure();
  report();
  send({kind:"fontsize", px});
}

// ── Remote history pager (phone only) ────────────────────────────────
// A full-screen TUI can't be scrolled smoothly over the network, so the phone
// turns history one screenful at a time. Two buttons (or a vertical swipe) each
// move one page — the whole screen minus a couple of kept rows, so the edge you
// were just reading carries over instead of vanishing. Rapid taps add up (shown
// as ×N) and fire as one move.
//
// Deliberately NO slide animation: over the wire it fought the periodic screen
// refresh and mostly stalled, and it added load for nothing. The frame just
// updates in place — which is what actually works reliably. Remote-only; the
// window keeps its native wheel.
// One tap moves about a THIRD of a screen (of this phone's height, gRows), not a
// whole one. Coalesced taps (×N) build up from there, so tapping once nudges and
// tapping a few times travels — finer control than a full page per tap.
const STEP_FRACTION = 3;
let pgPending = 0, pgTimer = 0, pgWaiting = false, pgWaitTimer = 0;

function pgReset() {
  pgPending = 0; pgWaiting = false; clearTimeout(pgTimer); clearTimeout(pgWaitTimer);
  const c = document.getElementById("pageCount"); if (c) c.textContent = "";
}
// The little indicator between the buttons: the pending count while you tap,
// then a spinner from the moment a move fires until the new screen lands. On a
// good link the spinner flashes for a fraction of a second; on a slow one it
// stays, so a tap never feels like it did nothing.
function pgCount() {
  const c = document.getElementById("pageCount");
  if (!c) return;
  if (pgPending !== 0) c.textContent = (pgPending > 0 ? "▲×" : "▼×") + Math.abs(pgPending);
  else if (pgWaiting) c.innerHTML = '<span class="pgspin"></span>';
  else c.textContent = "";
}
// The awaited screen arrived (called from the state socket / poll): stop waiting.
function pgArrived() {
  if (!pgWaiting) return;
  pgWaiting = false; clearTimeout(pgWaitTimer);
  pgCount();
}
// A tap adds to the pending count (shown as ×N) and re-arms a short timer, so a
// flurry of taps becomes one move of N pages instead of a stutter of round
// trips. dir +1 = older (into the past), -1 = newer (toward the present).
function pageBy(dir) {
  if (!document.getElementById("pageui").classList.contains("on")) return;
  // Reviewing history needs the whole screen — put the soft keyboard away
  // (the tap-to-focus handlers already skip the buttons, so it stays away).
  if (REMOTE && kbd) kbd.blur();
  pgPending += dir;
  pgCount();
  clearTimeout(pgTimer);
  pgTimer = setTimeout(pgFire, 160);
}
function pgFire() {
  if (pgPending === 0) return;
  const dir = pgPending > 0 ? 1 : -1;
  // How many pages this coalesced move covers. Capped so a flurry can't jump
  // across the whole history at once.
  const blocks = Math.min(8, Math.abs(pgPending));
  pgPending = 0; pgCount();
  if (!cellH) measure();
  // A fraction of a screen, in wheel ticks (~one tick per row). A FIXED step on
  // purpose: auto-tuning the tick rate per turn oscillated and flung the count
  // around. Fixed is deterministic — every tap moves the same amount; if it turns
  // out a touch too much or too little, it's one number (STEP_FRACTION) to nudge.
  const PAGE_STEP = Math.max(1, Math.round(gRows / STEP_FRACTION));
  const notches = Math.min(250, blocks * PAGE_STEP);
  send({kind:"scroll", by: dir > 0 ? notches : -notches, row: 0, col: 0});
  // The scrolled screen arrives on its own over the state socket (pgArrived
  // stops the wait). Show a spinner in the meantime so the gap never reads as a
  // dead tap; a safety timer clears it if a frame somehow never comes.
  pgWaiting = true;
  pgCount();
  clearTimeout(pgWaitTimer);
  pgWaitTimer = setTimeout(() => { pgWaiting = false; pgCount(); }, 3000);
}

document.getElementById("pageUp").addEventListener("click", () => pageBy(1));
document.getElementById("pageDown").addEventListener("click", () => pageBy(-1));

// A vertical swipe on the terminal pages once in that direction (down-swipe =
// older), same as a button tap. Continuous drag-scrolling is deliberately gone:
// it can't be smooth across the network, and coalesced page turns can.
let swY = null, swDist = 0;
scr.addEventListener("touchstart", e => {
  if (!REMOTE || !S || S.active === 0 || scr.hidden || e.touches.length !== 1) { swY = null; return; }
  swY = e.touches[0].clientY; swDist = 0;
}, {passive:true});
scr.addEventListener("touchmove", e => {
  if (swY === null || e.touches.length !== 1) return;
  swDist += e.touches[0].clientY - swY;
  swY = e.touches[0].clientY;
  if (Math.abs(swDist) > 10) e.preventDefault();   // claim the gesture from the page
}, {passive:false});
scr.addEventListener("touchend", () => {
  if (swY === null) return;
  const d = swDist; swY = null;
  if (Math.abs(d) > 40) pageBy(d > 0 ? 1 : -1);
}, {passive:true});
scr.addEventListener("touchcancel", () => { swY = null; }, {passive:true});

// ── Reader (phone only) ──────────────────────────────────────────────
// The words of this tab's conversation, as a document. Where they come from
// and why they cannot come from the screen is written at the top of reader.rs;
// the short of it is that a full-screen TUI keeps no scrollback, so the pager
// above does not scroll our copy of anything — it asks the CLI to scroll
// itself, one round trip at a time, over text already broken to the terminal's
// width. Here the phone holds the text, so scrolling is the browser's own.
//
// Nothing in here talks to the PC while you read. That is the whole point: a
// flick has to travel, and a flick that must ask a PC across the house how far
// it went never will.
let rdTab = 0, rdFrom = 0, rdMore = false, rdLoading = false, rdWasBusy = false;
const rdPanel = () => document.getElementById("reader");
const rdBodyEl = () => document.getElementById("rbody");
const rdIsOpen = () => rdPanel().classList.contains("on");

// Inline spans, built as nodes rather than assembled into innerHTML. What a
// conversation contains is not ours to trust, and a node can never be read
// back as markup
function rdInline(node, text) {
  const re = /`([^`]+)`|\*\*([^*]+)\*\*/g;
  let at = 0, m;
  while ((m = re.exec(text))) {
    if (m.index > at) node.append(document.createTextNode(text.slice(at, m.index)));
    node.append(m[1] != null ? el("code", {}, m[1]) : el("b", {}, m[2]));
    at = m.index + m[0].length;
  }
  if (at < text.length) node.append(document.createTextNode(text.slice(at)));
  return node;
}

// Enough Markdown for an answer to read as one: fenced code, headings, bullets,
// and the inline spans. Not a Markdown engine and not trying to become one —
// whatever it does not recognise it shows verbatim, which for a reader is the
// correct way to fail: the words still arrive
function rdMarkup(text) {
  const out = document.createDocumentFragment();
  const lines = String(text).split(/\r?\n/);
  const row = l => /^\s*\|.*\|\s*$/.test(l);
  const cells = l => l.trim().replace(/^\||\|$/g, "").split("|").map(c => c.trim());
  let i = 0, para = [], list = null;
  const endPara = () => { if (para.length) { out.append(rdInline(el("p", {}), para.join("\n"))); para = []; } };
  const endList = () => { if (list) { out.append(list); list = null; } };
  const intoList = (tag, body) => {
    endPara();
    if (list && list.tagName.toLowerCase() !== tag) endList();
    if (!list) list = el(tag, {});
    list.append(rdInline(el("li", {}), body));
  };
  while (i < lines.length) {
    const line = lines[i];
    if (/^\s*```/.test(line)) {
      endPara(); endList();
      const code = [];
      for (i++; i < lines.length && !/^\s*```/.test(lines[i]); i++) code.push(lines[i]);
      i++;   // step over the closing fence (or off the end, if it never came)
      out.append(el("pre", {}, el("code", {}, code.join("\n"))));
      continue;
    }
    // A table is a header row, the |---|---| beneath it, then the rows. Left as
    // pipes it reads as line noise, and an answer that compares things is
    // exactly the kind worth opening a reader for
    if (row(line) && /^\s*\|[\s:|-]+\|\s*$/.test(lines[i + 1] || "")) {
      endPara(); endList();
      const table = el("table", {});
      const head = el("tr", {});
      for (const c of cells(line)) head.append(rdInline(el("th", {}), c));
      table.append(head);
      for (i += 2; i < lines.length && row(lines[i]); i++) {
        const tr = el("tr", {});
        for (const c of cells(lines[i])) tr.append(rdInline(el("td", {}), c));
        table.append(tr);
      }
      out.append(el("div", {class:"rtable"}, table));
      continue;
    }
    const head = /^(#{1,6})\s+(.*)$/.exec(line);
    if (head) { endPara(); endList(); out.append(rdInline(el("h3", {}), head[2])); i++; continue; }
    const bullet = /^\s*[-*]\s+(.*)$/.exec(line);
    if (bullet) { intoList("ul", bullet[1]); i++; continue; }
    const numbered = /^\s*\d+[.)]\s+(.*)$/.exec(line);
    if (numbered) { intoList("ol", numbered[1]); i++; continue; }
    if (line.trim() === "") { endPara(); endList(); i++; continue; }
    endList(); para.push(line); i++;
  }
  endPara(); endList();
  return out;
}

function rdTurn(turn) {
  const you = turn.who === "you";
  const box = el("div", {class: "rturn " + (you ? "you" : "ai")});
  box.append(el("div", {class:"rwho"},
    you ? (T["tui.read.you"] || "YOU") : (T["tui.read.ai"] || "AI")));
  box.append(rdMarkup(turn.text));
  return box;
}

// Two blocks from the same speaker can land side by side, where a page had to
// stop part-way through a turn (a run of tool output longer than one read). Say
// the name once: that seam is ours, not something that happened in the
// conversation.
function rdRelabel() {
  let before = null;
  for (const turn of rdBodyEl().querySelectorAll(".rturn")) {
    const who = turn.classList.contains("you") ? "you" : "ai";
    const label = turn.querySelector(".rwho");
    if (label) label.style.display = who === before ? "none" : "";
    before = who;
  }
}

function rdNote(text) {
  const body = rdBodyEl();
  body.textContent = "";
  body.append(el("div", {class:"rwho"}, text));
}

// The opening page is deliberately small: the last exchange, and nothing else.
// Walking back is a tap, and each tap brings a handful
const RD_FIRST = 2, RD_MORE = 6;

async function rdAsk(before, want) {
  const q = "api/read?t=" + encodeURIComponent(TOKEN) + "&tab=" + rdTab + "&want=" + want
    + (before != null ? "&before=" + before : "");
  const r = await fetch(q, {cache:"no-store"});
  if (!r.ok) throw new Error("status " + r.status);
  return await r.json();
}

// Open at the TOP of the newest answer, never at its end. "Scrolled to the
// bottom" is where a terminal leaves you and is precisely the complaint: the
// head of a long answer is the part that never survives
function rdToLatest() {
  const body = rdBodyEl();
  const said = body.querySelectorAll(".rturn.ai");
  const last = said[said.length - 1];
  if (!last) { body.scrollTop = body.scrollHeight; return; }
  body.scrollTop = Math.max(0,
    body.scrollTop + last.getBoundingClientRect().top - body.getBoundingClientRect().top - 8);
}

// The way back into the past: one deliberate tap, at the head of the document
// where the conversation actually continues upward. Not an automatic load on
// reaching the top — the reader opens on the last exchange on purpose, and
// something that quietly kept pulling more in would undo that.
const rdEarlier = el("button", { id: "rearlier", onclick: () => rdOlder() });

function rdEarlierState() {
  rdEarlier.style.display = rdMore ? "block" : "none";
  rdEarlier.textContent = rdLoading
    ? (T["tui.read.loading"] || "…")
    : (T["tui.read.earlier"] || "");
  rdEarlier.disabled = rdLoading;
}

async function rdOlder() {
  if (rdLoading || !rdMore) return false;
  rdLoading = true;
  rdEarlierState();
  const body = rdBodyEl();
  try {
    const j = await rdAsk(rdFrom, RD_MORE);
    if (!j.ok) { rdMore = false; return false; }
    rdFrom = j.from; rdMore = !!j.more;
    if (!j.turns || !j.turns.length) return rdMore;
    const frag = document.createDocumentFragment();
    for (const turn of j.turns) frag.append(rdTurn(turn));
    // In after the button, before everything already read
    const first = body.querySelector(".rturn");
    body.insertBefore(frag, first);
    // Land on what was just fetched. They asked to see it — leaving them where
    // they stood, with the new material somewhere above, would make the tap
    // look like it did nothing
    rdRelabel();
    const added = body.querySelector(".rturn");
    if (added) {
      body.scrollTop = Math.max(0,
        body.scrollTop + added.getBoundingClientRect().top - body.getBoundingClientRect().top - 8);
    }
    return true;
  } catch (e) {
    return false;
  } finally {
    rdLoading = false;
    rdEarlierState();
  }
}

async function rdShow() {
  const tab = activeTab();
  if (!REMOTE || !tab || !tab.readable) return;
  rdTab = tab.index;
  rdFrom = 0; rdMore = false; rdWasBusy = false;
  document.getElementById("rname").textContent = tab.name || "";
  document.getElementById("rmore").classList.remove("on");
  rdNote(T["tui.read.loading"] || "…");
  rdPanel().classList.add("on");
  document.body.classList.add("reading");
  // Reading is reading: put the keyboard away and stop the screen relay's
  // gestures from being aimed at a tab nobody is looking at
  if (kbd) kbd.blur();
  try {
    const j = await rdAsk(null, RD_FIRST);
    if (!j.ok || !j.turns || !j.turns.length) {
      rdNote(T["tui.read.empty"] || "");
      return;
    }
    rdFrom = j.from; rdMore = !!j.more;
    const body = rdBodyEl();
    body.textContent = "";
    body.append(rdEarlier);
    rdEarlierState();
    for (const turn of j.turns) body.append(rdTurn(turn));
    rdRelabel();
    rdToLatest();
  } catch (e) {
    rdNote(String((e && e.message) || e));
  }
}

function rdHide() {
  rdPanel().classList.remove("on");
  document.body.classList.remove("reading");
  rdBodyEl().textContent = "";
  document.getElementById("rfoot").classList.remove("on");
  document.getElementById("rmore").classList.remove("on");
}

// Called on every state change. While a turn is running the terminal is what
// you watch it arrive on — that live screen IS the loading indicator, and this
// says so rather than pretending the record is behind. When the turn lands,
// nothing moves on its own: a reader mid-sentence is not to be yanked
// somewhere else, so the new answer is offered as a button
function rdOnState() {
  if (!rdIsOpen()) return;
  // The reader belongs to the tab it was opened from. Follow a tab switch and
  // it would quietly show somebody else's conversation under the same heading
  if (S && S.active !== rdTab) { rdHide(); return; }
  const tab = (S && S.tabs) ? S.tabs.find(t => t.index === rdTab) : null;
  const busy = !!(tab && (tab.state === "BUSY" || tab.busy));
  const foot = document.getElementById("rfoot");
  foot.classList.toggle("on", busy);
  foot.textContent = busy ? (T["tui.read.generating"] || "") : "";
  if (rdWasBusy && !busy) {
    const more = document.getElementById("rmore");
    more.textContent = T["tui.read.arrived"] || "";
    more.classList.add("on");
  }
  rdWasBusy = busy;
}

document.getElementById("readOpen").addEventListener("click", () => rdShow());
document.getElementById("rclose").addEventListener("click", () => rdHide());
document.getElementById("rmore").addEventListener("click", () => rdShow());


// The top bar's input needs to behave like an ordinary text field. If
// merely selecting text copied it, or right-click pasted into the
// terminal, editing the URL would be impossible
// Taps on the top bar or the page buttons must not pull focus into the hidden
// #kbd — on a phone that would pop the soft keyboard up over the screen.
// The ✏️ pen counts as "the bar": otherwise the phone's tap-on-terminal rule
// below opened the bar on mouseup and the pen's own click toggled it shut again
const inBar = e => e.target && e.target.closest && e.target.closest("#nav, #pageui, #castdock, #composerfab, #reader");
document.addEventListener("mouseup", e => {
  if (inBar(e)) return;
  const s = window.getSelection();
  const t = s ? s.toString() : "";
  if (t) { send({kind:"copy", text:t}); return; }
  // On a phone, tapping a terminal tab opens the sub-input bar (see openTermBar)
  // rather than the hidden #kbd, so the keyboard never lands on top of the screen.
  //
  // Except after the person's own ✕. That press means "out of my way", and a tap
  // used to undo it on the spot — so the bar came straight back the moment the
  // screen was touched, and the ✕ meant nothing. The ✎ pen is the way back in.
  if (REMOTE && onTermPty()) { if (!castClosed()) openTermBar(); return; }
  focus();
});
document.addEventListener("contextmenu", e => {
  if (inBar(e)) return;
  e.preventDefault();
  send({kind:"paste"});
  focus();
});
window.addEventListener("focus", focus);
focus();
measure();
report();

// The window is handed its state directly. A phone instead receives it PUSHED
// over a WebSocket the moment the screen or UI changes — no constant polling, so
// it's quiet when idle and updates instantly when active (a scrolled page, a new
// line of output). If the socket can't hold — a flaky link, an older server — a
// slow poll takes over until it reconnects.
if (REMOTE) {
  let wsUp = false, sws = null, downSince = 0;
  // Say what happened when the feed stops. The PC can end this session
  // deliberately (its "disconnect": every request then answers 403 and this
  // page is done until someone opens the link again), or the link can simply
  // drop. Either way the stale screen would otherwise just sit there looking
  // live — which is the worst of the three states, because it is the one that
  // gets acted on.
  const showNet = (kind) => {
    let v = document.getElementById("netveil");
    if (!kind) { if (v) v.hidden = true; return; }
    if (!v) {
      v = el("div", {id:"netveil"},
        el("div", {class:"nvbox"},
          el("div", {class:"nvicon"}, kind === "cut" ? "⛔" : "⚠"),
          el("div", {class:"nvtitle"}, ""),
          el("div", {class:"nvsub"}, ""),
          // Only a fixed pairing can come back on its own: the token is
          // unchanged, so opening the link again is all it takes. A rotated
          // token leaves nothing to press — the new QR is the only way in.
          STICKY && TOKEN
            ? el("button", {class:"nvbtn", onclick:() => {
                location.href = "/?t=" + encodeURIComponent(TOKEN);
              }}, T["tui.net.cut.again"] || "Reconnect")
            : null));
      document.body.append(v);
    }
    v.hidden = false;
    v.classList.toggle("cut", kind === "cut");
    v.querySelector(".nvicon").textContent = kind === "cut" ? "⛔" : "⚠";
    v.querySelector(".nvtitle").textContent = kind === "cut"
      ? (T["tui.net.cut.title"] || "Disconnected from this PC")
      : (T["tui.net.down.title"] || "Connection lost");
    v.querySelector(".nvsub").textContent = kind === "cut"
      ? (STICKY
          ? (T["tui.net.cut.sub.sticky"] || "The PC ended this session. Its access code is unchanged, so opening the link again reconnects this device.")
          : (T["tui.net.cut.sub"] || "The PC ended this session (its access code changed). Scan the new QR code on the PC to reconnect."))
      : (T["tui.net.down.sub"] || "Reconnecting…");
    const btn = v.querySelector(".nvbtn");
    if (btn) btn.hidden = kind !== "cut";
  };
  // The PC ended this session. Stop everything this page holds — the state
  // socket and, above all, the relay: its input line is a separate socket, and
  // a page that keeps it open keeps a finger on a browser it can no longer see.
  const cutNow = () => {
    if (remoteCut) return;
    remoteCut = true; wsUp = false;
    try { if (sws) sws.close(); } catch (x) {}
    castStop();
    showNet("cut");
  };
  const connected = () => { downSince = 0; showNet(null); };
  const applyState = (d) => {
    // The PC says the line is closed. It says so before dropping the socket, so
    // the screen goes dark the moment the person there decides it does, instead
    // of at the next poll.
    if (d.cut) { cutNow(); return; }
    connected();
    if (d.ui) window.__state(typeof d.ui === "string" ? d.ui : JSON.stringify(d.ui));
    if (d.screen_html != null) { window.__screen(d.screen_html); pgArrived(); }
    // 📼 pushes: a recorded Lua line for the composer, or a ▶ run's verdict
    // (null = clean, so test for the key's presence, not its truthiness).
    if (d.recorded != null) window.__recorded(d.recorded);
    if ("luadone" in d) window.__luaDone(d.luadone);
    if ("suggested" in d) window.__suggested(d.suggested);
    if ("surveyed" in d) window.__surveyed(d.surveyed);
  };
  const connectState = () => {
    if (remoteCut) return;
    try {
      const proto = location.protocol === "https:" ? "wss:" : "ws:";
      sws = new WebSocket(proto + "//" + location.host + "/ws-state?t=" + encodeURIComponent(TOKEN));
    } catch (e) { setTimeout(connectState, 1500); return; }
    sws.onopen = () => { wsUp = true; connected(); };
    sws.onmessage = (e) => { try { applyState(JSON.parse(e.data)); } catch (x) {} };
    sws.onclose = () => { wsUp = false; if (!remoteCut) setTimeout(connectState, 1500); };
    sws.onerror = () => { try { sws.close(); } catch (x) {} };
  };
  connectState();
  // Fallback poll — only does anything while the socket is down. It's also the
  // reliable place to notice a revoked token: a WS handshake failure is opaque,
  // but a plain fetch returns the 403 outright.
  const pull = async () => {
    if (wsUp || remoteCut) return;
    try {
      const r = await fetch("api/state?t=" + encodeURIComponent(TOKEN), {cache:"no-store"});
      if (r.status === 403) {
        // Two different refusals share the status: the optional password
        // gate (body "password") wants the person to unlock this device
        // once; anything else means the PC ended this session.
        const body = await r.text().catch(() => "");
        if (body === "password") {
          const pw = prompt(T["tui.remote.password_prompt"] || "パスワード");
          if (pw !== null && pw !== "") {
            const a = await fetch("auth?t=" + encodeURIComponent(TOKEN) + "&p=" + encodeURIComponent(pw), {cache:"no-store"});
            if (a.ok) { location.reload(); return; }
            alert(T["tui.remote.password_wrong"] || "パスワードが違います");
          }
          return;
        }
        cutNow();   // this session was ended at the PC — the page is done
        return;
      }
      if (!r.ok) throw new Error("status " + r.status);
      applyState(await r.json());
    } catch (e) {
      // A plain network drop. Give the socket a couple of seconds to reconnect
      // before crying wolf — a one-frame blip shouldn't flash a scary banner.
      if (!downSince) downSince = Date.now();
      else if (Date.now() - downSince > 3000) showNet("down");
    }
  };
  pull();
  setInterval(pull, 1500);
}

// ── Screen relay (viewing and touching a browser tab from a phone) ──────────────
// Frames come down over /ws; finger input goes up over /ws-in. Keeping
// them as two separate one-directional sockets means neither one can
// clog the other. Coordinates are sent as a 0..1 fraction, independent of the device's screen size
let castWs = null, castIn = null, castCtx = null, castBound = false;
// The screen shape already reported to the PC (it re-shapes the page's
// viewport to match, so a portrait phone gets a full screen, not a
// letterboxed strip). Width-keyed: the keyboard opening only changes the
// height, and re-shaping the page for that would make it jump around
let castShaped = false, shapeW = 0, shapeT = 0;
function sendShape(force) {
  const cv = document.getElementById("cast");
  const w = Math.round(cv.clientWidth), h = Math.round(cv.clientHeight);
  if (!w || !h) return false;
  if (!force && w === shapeW) return true;
  if (!sendIn({kind:"inject", what:"view", w:w, h:h})) return false;
  shapeW = w;
  return true;
}
function castStart() {
  if (!REMOTE || castWs || remoteCut) return;
  const cv = document.getElementById("cast");
  castCtx = cv.getContext("2d");
  const proto = location.protocol === "https:" ? "wss:" : "ws:";
  const base = proto + "//" + location.host;
  const tok = encodeURIComponent(TOKEN);
  castWs = new WebSocket(base + "/ws?t=" + tok);
  castWs.binaryType = "blob";
  castWs.onmessage = async (e) => {
    try {
      const bmp = await createImageBitmap(e.data);
      if (cv.width !== bmp.width || cv.height !== bmp.height) {
        cv.width = bmp.width; cv.height = bmp.height;
        // If a frame changes the canvas dimensions, recompute the cursor
        // position too (before the first frame it defaults to 300x150, which throws things off)
        if (castMode) posCursor();
        // A new frame shape = a different page is being cast (tab switch) or
        // its window was resized — report our screen shape again. Re-reporting
        // is idempotent on the PC side, so this can't ping-pong
        castShaped = false;
      }
      // Report the screen shape only once a frame exists: the PC computes
      // the new viewport from the current one, so it must have seen a frame
      if (!castShaped) castShaped = sendShape(true);
      castCtx.drawImage(bmp, 0, 0);
      if (bmp.close) bmp.close();
    } catch (err) {}
  };
  castWs.onclose = () => { castWs = null; };
  castIn = new WebSocket(base + "/ws-in?t=" + tok);
  castIn.onclose = () => { castIn = null; };
  bindCastInput(cv);
}
// Rotating the phone changes the width — tell the PC the new shape (debounced)
window.addEventListener("resize", () => {
  if (!castWs) return;
  clearTimeout(shapeT);
  shapeT = setTimeout(() => { if (castWs) sendShape(false); }, 300);
});
function castStop() {
  if (castWs) { castWs.close(); castWs = null; }
  if (castIn) { castIn.close(); castIn = null; }
  castShaped = false; shapeW = 0;
  zoomReset();
  // Only tear down browser CONTROL mode. On a terminal tab castMode is already
  // false, and its sub-input bar must survive the per-update __state redraws
  // (this runs on every terminal tab, once per state push) — closing it here
  // would make the bar vanish the moment any output arrived.
  if (castMode) exitCast();
}
function sendIn(o) {
  if (castIn && castIn.readyState === 1) { castIn.send(JSON.stringify(o)); return true; }
  return false;
}
// Are we sending into a browser right now? The phone's relay (castMode) and the
// window's native browser tab are the same case — both inject into the shown page.
function drivingBrowser() { return castMode || onBrowserTab(); }
// Inject an intent into the shown browser. The phone has a low-latency relay
// socket (sendIn); the window has none, so the identical intent rides ipc (send)
// instead, reaching the very same caps.browser_inject. One path, two transports.
function injectIn(o) { if (!sendIn(o)) send(o); }
// ── Pinch zoom of the relay view ──────────────
// Display-side magnification only (the page itself is untouched): a CSS
// transform on the canvas. castRect() reads getBoundingClientRect(), which
// already reflects transforms, so cursor/tap coordinate math needs nothing extra
let zs = 1, zx = 0, zy = 0;
function applyZoom() {
  const cv = document.getElementById("cast");
  if (zs <= 1.001) { zs = 1; zx = 0; zy = 0; cv.style.transform = ""; return; }
  // Keep the view inside the canvas box: no gap may open on any edge
  const mw = cv.clientWidth, mh = cv.clientHeight;
  zx = Math.min(0, Math.max(mw - mw * zs, zx));
  zy = Math.min(0, Math.max(mh - mh * zs, zy));
  cv.style.transform = "translate(" + zx + "px," + zy + "px) scale(" + zs + ")";
}
function zoomReset() {
  zs = 1; zx = 0; zy = 0;
  const cv = document.getElementById("cast");
  if (cv) cv.style.transform = "";
}
// Returns the letterboxed content rect (from object-fit:contain) in client
// coordinates. The image uses object-position:top center, so it's
// horizontally centered but top-aligned vertically. Using a vertically
// centered calculation here would throw off the cursor and click position vertically
function castRect(cv) {
  const r = cv.getBoundingClientRect();
  const cw = cv.width || 1, ch = cv.height || 1;
  const s = Math.min(r.width / cw, r.height / ch);
  const dw = cw * s, dh = ch * s;
  return { ox: r.left + (r.width - dw) / 2, oy: r.top, dw, dh };
}

// Trackpad-style cursor.
//   1) Tap the cast → enters control mode and shows the cursor (this tap itself doesn't click)
//   2) Drag → moves the cursor relatively (the finger never covers the
//      target, so even small targets can be hit)
//   3) Tap → clicks at the cursor position
//   4) Tap a second time quickly, then move → grab-and-drag (e.g. a CAPTCHA slider)
//   5) Two-finger drag → scroll
//   6) Tap the top bar or the control-mode badge → release
let castMode = false, cx = 0.5, cy = 0.5, cursorEl = null, modeEl = null, dragging = false;
let modCtrl = false, modAlt = false;   // Ctrl/Alt latching toggles
const CURSOR_ACCEL = 1.25;
function clamp01(v) { return v < 0 ? 0 : v > 1 ? 1 : v; }
function ensureCursor() {
  if (!cursorEl) {
    cursorEl = el("div", {id:"castcursor"});
    // An arrow shaped like the standard Windows cursor; its tip (viewBox
    // coordinates 1,1) is the click point. A negative CSS margin aligns
    // the tip exactly with left/top
    cursorEl.innerHTML = '<svg width="19" height="30" viewBox="0 0 12 19">' +
      '<path d="M1 1 L1 15 L4.5 11.5 L7 17 L9 16 L6.5 10.5 L11 10.5 Z" ' +
      'fill="#000" stroke="#fff" stroke-width="1" stroke-linejoin="round"/></svg>';
    document.getElementById("main").append(cursorEl);
  }
}
// Show a ripple at the click location (confirms the tap registered).
// Computed from castRect() so it lands right even while pinch-zoomed
function spawnRipple() {
  const cv = document.getElementById("cast");
  const m = document.getElementById("main").getBoundingClientRect();
  const c = castRect(cv);
  const r = el("div", {class:"ripple"});
  r.style.left = (c.ox - m.left + cx * c.dw) + "px";
  r.style.top = (c.oy - m.top + cy * c.dh) + "px";
  document.getElementById("main").append(r);
  setTimeout(() => r.remove(), 480);
}
// Text input bar. Shows a preview input field at the bottom of the screen
// where IME conversion (e.g. Japanese) can be watched while typing, then
// sent all at once with "Send" once confirmed. This has three benefits:
//   - the in-progress conversion is visible in its own field (so it can be
//     fixed up before sending)
//   - the relayed screen stays visible above the bar, so the target never gets lost
//   - the keyboard appears below the bar (visualViewport lifts the bar above it)
// Display labels for the auxiliary keys. Names with no label fall back to their uppercased form
const CAST_LABEL = {
  esc:"Esc", tab:"Tab", space:"Space", enter:"⏎", backspace:"⌫", delete:"Del",
  left:"←", up:"↑", down:"↓", right:"→",
  home:"Home", end:"End", pageup:"PgUp", pagedown:"PgDn", ctrl:"Ctrl", alt:"Alt" };
function castKeyLabel(name) { return CAST_LABEL[name] || name.toUpperCase(); }
// Cast key names → the terminal's own named-key vocabulary (what the #kbd path
// sends). Anything not listed passes through unchanged (esc/tab/enter/arrows/home/end).
const TERM_KEY = { backspace:"bs", delete:"del", pageup:"pgup", pagedown:"pgdn" };
// Send a single auxiliary key press. In browser control mode it goes to the relay
// (latching Ctrl/Alt combine in); over a terminal it sends the very intents #kbd
// would send. Either way, any latch is released afterwards.
function sendCastKey(name) {
  if (drivingBrowser()) {
    injectIn({kind:"inject", what:"key", named:name, ctrl:modCtrl, alt:modAlt});
  } else if (name === "space") {
    send({kind:"key", text:" "});
  } else if (name !== "ctrl" && name !== "alt") {
    send({kind:"key", named: TERM_KEY[name] || name});
  }
  if (modCtrl || modAlt) { modCtrl = false; modAlt = false; refreshMods(); }
}
// Sync the latching toggles' appearance with their current state
function refreshMods() {
  if (!castKeysEl) return;
  castKeysEl.querySelectorAll(".castkey.mod").forEach(b => {
    const on = (b.dataset.k === "ctrl" && modCtrl) || (b.dataset.k === "alt" && modAlt);
    b.classList.toggle("on", on);
  });
}
function buildCastKeys() {
  const row = el("div", {id:"castkeys"});
  const keys = (CAST_KEYS && CAST_KEYS.length) ? CAST_KEYS
    : ["esc","tab","left","up","down","right","space","enter","backspace"];
  keys.forEach(name => {
    const isMod = (name === "ctrl" || name === "alt");
    const b = el("button", {class:"castkey" + (isMod ? " mod" : ""), "data-k":name}, castKeyLabel(name));
    // Prevent the default action on pointerdown, so the input field keeps focus (i.e. the keyboard stays open)
    b.addEventListener("pointerdown", (e) => e.preventDefault());
    b.onclick = (e) => {
      e.stopPropagation();
      if (isMod) {
        if (name === "ctrl") modCtrl = !modCtrl; else modAlt = !modAlt;
        refreshMods();
      } else { sendCastKey(name); }
    };
    row.append(b);
  });
  return row;
}
// The quick-actions row for the sub-input bar. A text action inserts its string
// into the composer on click (a reviewable "auto-fill", not an immediate send);
// Lua actions are carried but not fired yet, so only text ones are shown for now
// (no dead buttons). Returns null when there are none, so the row is omitted.
function buildActions() {
  // Keep each action's original index (Lua actions are fired server-side by it).
  const items = (curActions || []).map((a, i) => ({a, i})).filter(x => x.a && (x.a.text != null || x.a.lua));
  if (!items.length) return null;
  const row = el("div", {id:"castactions"});
  items.forEach(({a, i}) => {
    const isLua = !!a.lua;
    const b = el("button", {class:"castaction" + (isLua ? " lua" : ""),
      title: isLua ? (a.label || "") : (a.text || "")}, a.label || a.text || "•");
    // Keep the composer's focus (= the keyboard) when tapping an action.
    b.addEventListener("pointerdown", (e) => e.preventDefault());
    b.onclick = (e) => {
      e.stopPropagation();
      if (isLua) {
        // The code lives server-side; fire it by index and note that it ran.
        send({kind:"runaction", index: i});
        toast("▶ " + (a.label || ""));
      } else {
        if (!castInput) return;
        const cur = castInput.value;
        const sep = (cur && !cur.endsWith(" ") && !cur.endsWith("\n")) ? " " : "";
        castInput.value = cur + sep + a.text;
        castInput.focus();
        growCastInput();
      }
    };
    row.append(b);
  });
  return row;
}
// True while the window is showing a browser tab.
function onBrowserTab() { return S && S.tabs && S.tabs.some(t => t.index === S.active && t.kind === "browser"); }

// The config changed (a settings save): swap in the new actions and re-render the
// panel if it's the one showing, so an edit reflects without reloading the window.
// Swap the whole palette without reloading. Everything the window draws with
// is a variable, so a scheme change is one rule being replaced -- including the
// terminal's own sixteen, which the cells name rather than carry.
// The Vault overlay: search past conversations, reopen one as a resuming tab.
//
// Opening it asks for the recent ones (a blank search). Typing narrows, with a
// short pause so a search does not fire on every letter. The results arrive in
// the state (S.vault), so the same overlay works from the phone -- the window
// runs the search and both sides read the answer
let vaultTimer = 0;
window.__openVault = function () {
  const v = document.getElementById("vault");
  if (!v) return;
  v.hidden = false;
  const q = document.getElementById("vq");
  q.placeholder = T["vault.placeholder"] || "Search past conversations…";
  q.value = "";
  v.querySelector(".vtitle").textContent = T["vault.title"] || "PAST WORK";
  renderVault();
  send({kind:"vaultsearch", query:""});
  setTimeout(() => q.focus(), 30);
};
function closeVault() {
  const v = document.getElementById("vault");
  if (v) v.hidden = true;
}
// How long ago, in the plainest words a row has space for
function ago(sec) {
  if (!sec) return "";
  const d = Math.max(0, Math.floor(Date.now()/1000) - sec);
  if (d < 3600) return Math.max(1, Math.floor(d/60)) + "m";
  if (d < 86400) return Math.floor(d/3600) + "h";
  if (d < 86400*30) return Math.floor(d/86400) + "d";
  return Math.floor(d/(86400*30)) + "mo";
}
function renderVault() {
  const v = document.getElementById("vault");
  if (!v || v.hidden) return;
  const list = v.querySelector(".vlist");
  const hint = v.querySelector(".vhint");
  const vs = S && S.vault;
  list.textContent = "";
  const hits = (vs && vs.hits) || [];
  if (!hits.length) {
    hint.textContent = T["vault.none"] || "Nothing found.";
    return;
  }
  hint.textContent = vs.capped
    ? (T["vault.more"] || "Showing the most recent matches — narrow the search for older ones.")
    : "";
  for (const h of hits) {
    // A live hit is a line in an open tab: selecting it goes to that tab. A
    // past hit is a record: selecting it reopens the conversation
    const live = (h.tab !== undefined && h.tab !== null);
    const row = el("div", {class:"vrow", onclick:() => {
      closeVault();
      if (live) send({kind:"select", tab:h.tab});
      else send({kind:"vaultopen", program:h.program, id:h.id, cwd:h.cwd || "", title:h.title});
    }});
    row.append(el("div", {class:"vr1"},
      el("span", {class:"vprog"}, live ? (T["vault.live"] || "open") : h.program),
      el("span", {class:"vname"}, h.title),
      el("span", {class:"vwhen"}, live ? (T["vault.here"] || "on screen") : ago(h.when))));
    if (h.snippet) row.append(el("div", {class:"vsnip"}, h.snippet));
    list.append(row);
  }
}
// The input and the overlay's own keys, wired once
(function () {
  const q = document.getElementById("vq");
  if (q) {
    q.addEventListener("input", () => {
      clearTimeout(vaultTimer);
      const query = q.value;
      vaultTimer = setTimeout(() => send({kind:"vaultsearch", query}), 180);
    });
  }
  const v = document.getElementById("vault");
  if (v) {
    v.querySelector(".vclose").addEventListener("click", closeVault);
    v.addEventListener("keydown", (e) => { if (e.key === "Escape") { e.preventDefault(); closeVault(); } });
    // A click on the dark surround (not the box) closes it
    v.addEventListener("mousedown", (e) => { if (e.target === v) closeVault(); });
  }
})();
// The command palette: find and run anything by typing. One list over the
// things a person reaches for -- go somewhere, do something, run one of their
// own actions -- filtered as they type, run on Enter or a click.
let palItems = [], palSel = 0;
window.__openPalette = function () {
  const v = document.getElementById("palette");
  if (!v) return;
  v.hidden = false;
  v.querySelector(".vtitle").textContent = T["palette.title"] || "COMMANDS";
  const q = document.getElementById("pq");
  q.placeholder = T["palette.placeholder"] || "Type a command…";
  q.value = "";
  buildPalette("");
  setTimeout(() => q.focus(), 30);
};
function closePalette() {
  const v = document.getElementById("palette");
  if (v) v.hidden = true;
}
function insertComposer(text) {
  if (typeof castInput !== "undefined" && castInput) {
    const cur = castInput.value;
    const sep = (cur && !/\s$/.test(cur)) ? " " : "";
    castInput.value = cur + sep + text;
    castInput.focus();
  }
}
// Everything runnable, freshly gathered each open so tabs and quick actions
// are current. Each item knows its own group and how to run itself
function paletteAll() {
  const out = [];
  out.push({grp:"go", label:T["tui.menu.settings"] || "Settings", run:() => openSettings()});
  out.push({grp:"go", label:T["tui.menu.vault"] || "Find past work", run:() => window.__openVault()});
  for (const t of (S && S.tabs || [])) {
    out.push({grp:"tab", label:(T["palette.gototab"] || "Go to") + " " + t.name,
      run:() => send({kind:"select", tab:t.index})});
  }
  for (const a of (typeof KEY_ACTIONS !== "undefined" ? KEY_ACTIONS : [])) {
    out.push({grp:"do", label:a.label, run:() => send({kind:"runkey", name:a.name})});
  }
  (typeof curActions !== "undefined" && curActions || []).forEach((a, i) => {
    if (!a) return;
    if (a.lua) out.push({grp:"run", label:a.label || "action", run:() => send({kind:"runaction", index:i})});
    else if (a.text != null) out.push({grp:"run", label:a.label || a.text, run:() => insertComposer(a.text)});
  });
  return out;
}
function buildPalette(query) {
  const q = (query || "").trim().toLowerCase();
  const all = paletteAll();
  // Plain contains-match, in the order the groups were built -- predictable is
  // more useful here than clever ranking
  palItems = q ? all.filter(it => it.label.toLowerCase().includes(q)) : all;
  palSel = 0;
  renderPalette();
}
function renderPalette() {
  const v = document.getElementById("palette");
  if (!v || v.hidden) return;
  const list = v.querySelector(".vlist");
  const grpName = g => T["palette.grp." + g] || g;
  list.textContent = "";
  palItems.forEach((it, i) => {
    const row = el("div", {class:"prow" + (i === palSel ? " sel" : ""),
      onclick:() => { closePalette(); it.run(); }});
    row.append(el("span", {class:"pgrp"}, grpName(it.grp)), el("span", {class:"plabel"}, it.label));
    list.append(row);
  });
  const sel = list.children[palSel];
  if (sel) sel.scrollIntoView({block:"nearest"});
}
(function () {
  const q = document.getElementById("pq");
  const v = document.getElementById("palette");
  if (!q || !v) return;
  q.addEventListener("input", () => buildPalette(q.value));
  q.addEventListener("keydown", (e) => {
    if (e.key === "ArrowDown") { e.preventDefault(); palSel = Math.min(palSel + 1, palItems.length - 1); renderPalette(); }
    else if (e.key === "ArrowUp") { e.preventDefault(); palSel = Math.max(palSel - 1, 0); renderPalette(); }
    else if (e.key === "Enter") { e.preventDefault(); const it = palItems[palSel]; if (it) { closePalette(); it.run(); } }
    else if (e.key === "Escape") { e.preventDefault(); closePalette(); }
  });
  v.querySelector(".vclose").addEventListener("click", closePalette);
  v.addEventListener("mousedown", (e) => { if (e.target === v) closePalette(); });
})();
window.__setTheme = function (vars, light) {
  document.getElementById("theme").textContent =
    ":root{" + vars + "color-scheme:" + (light ? "light" : "dark") + ";}";
};
window.__setActions = function (arr) {
  try {
    curActions = arr || [];
    if (castPanel === "actions" && castDock && castDock.style.display === "flex") renderPanel();
  } catch (e) {}
};
// Base64-encode an ArrayBuffer without blowing the call stack on large files.
function bufToB64(buf) {
  const bytes = new Uint8Array(buf);
  let bin = "";
  const CHUNK = 0x8000;
  for (let i = 0; i < bytes.length; i += CHUNK) {
    bin += String.fromCharCode.apply(null, bytes.subarray(i, i + CHUNK));
  }
  return btoa(bin);
}
// Save a picked/pasted/dropped file next to the active tab, then drop the saved
// path into the composer. The tab a file belongs to is whichever one the bar is
// over (S.active); the server writes it and hands back an absolute path.
function insertIntoComposer(path) {
  if (!castInput) return;
  const cur = castInput.value;
  castInput.value = (cur && !cur.endsWith(" ") && !cur.endsWith("\n") ? cur + " " : cur) + path + " ";
  castInput.focus();
  growCastInput();
}
// Size the sub-input textarea to its content, up to the CSS max-height (then it
// scrolls). Called on typing and after any programmatic value change.
function growCastInput() {
  if (!castInput) return;
  castInput.style.height = "auto";
  castInput.style.height = Math.min(castInput.scrollHeight, Math.round(window.innerHeight * 0.4)) + "px";
  // Over a browser tab the dock's height decides how much of #page the native
  // browser may cover — a grown textarea must push that reserve up too, or the
  // panel row slides in under the browser layer and can't be clicked.
  if (typeof syncBrowserReserve === "function") syncBrowserReserve();
}
// The window has no remote HTTP server, so it saves over the ipc bridge: post the
// bytes, and Rust replies by eval-ing window.__attachDone(id, result). Correlate
// the async reply by id.
let __attachSeq = 0;
const __attachPending = {};
window.__attachDone = function (id, result) {
  const cb = __attachPending[id];
  if (cb) { delete __attachPending[id]; cb(result); }
};
function attachViaIpc(name, data) {
  return new Promise((res) => {
    const id = ++__attachSeq;
    __attachPending[id] = res;
    setTimeout(() => {
      if (__attachPending[id]) { delete __attachPending[id]; res({ ok: false, error: T["attach.err.failed"] || "timeout" }); }
    }, 20000);
    send({ kind: "attach", id: id, name: name, data: data });
  });
}
// Save a picked/pasted file next to the active tab, then drop the saved path into
// the composer. The phone posts to the /api/attach HTTP route; the window hands
// it to Rust over ipc. Either way the tab is the active one the bar sits over.
async function attachFile(file) {
  if (!file) return;
  const tab = (S && S.active) || 0;
  if (!tab) { toast(T["attach.err.no_tab"] || "Pick a tab first", true); return; }
  toast("…");
  try {
    const name = file.name || "file";
    const data = bufToB64(await file.arrayBuffer());
    let j;
    if (REMOTE) {
      const r = await fetch("api/attach?t=" + encodeURIComponent(TOKEN), {
        method: "POST", body: JSON.stringify({ tab: tab, name: name, data: data })
      });
      j = await r.json();
    } else {
      j = await attachViaIpc(name, data);
    }
    if (j && j.ok && j.path) {
      insertIntoComposer(j.path);
      toast((T["attach.saved"] || "Attached {name}").replace("{name}", name));
    } else {
      toast((j && j.error) || T["attach.err.failed"] || "Attach failed", true);
    }
  } catch (e) {
    toast(T["attach.err.failed"] || "Attach failed", true);
  }
}
let castDock = null, castBar = null, castInput = null, castKeysEl = null, castAttEl = null, castSendEl = null;
// The active tab the 🎯 target panel was last built for, so __state can rebuild it
// when the operator changes (its enabled/disabled gate depends on that tab).
let lastCastActive = null;
// The bar's upper area is a single switchable panel: a fixed switcher on the left
// picks what fills the (horizontally scrolling) rest — the special keys, the quick
// actions, or (later) the operate-target picker. Default: keys on the phone (no
// physical keyboard), actions on the desktop.
let castPanel = null, castPanelEl = null;
// The panel the PERSON last picked. Renders fall back when a tab switch makes
// it unavailable, but never overwrite this — only an explicit pick does
let userPanel = null;
// 📼's chosen mode ("rec" | "run"). "run" until the user opts into recording —
// arming a recorder is never a side effect of merely opening the panel.
let luaMode = "run";
// The composer is ONE box holding TWO documents: the ordinary draft (text
// bound for the page/terminal — on the phone it's the only way to type into
// a browser) and 📼's Lua sheet (recorded steps; editable, runnable,
// copyable). The sheet is loaded ONLY in ▶ run mode. While ⏺ recording, the
// composer stays the ordinary input — typing into the page keeps working AND
// that very input is what gets recorded. Same rule on every surface.
let luaSheet = "", castDraft = "", castSlot = "draft";
// The tab the "operate" (🎯) panel is aimed at, or null. Step 1 = choosing it;
// the operate engine (the active AI writes Lua to drive it) is layered on next.
let castTarget = null;
// Open the settings screen: the child WebView in the window, or the reverse-proxied
// /cfg page (native + responsive) on the phone, handing the token over once.
// section: deep-link to one settings card (e.g. "actions"). ret: come back to the
// board once it's saved. Both optional — the sidebar gear passes neither.
function openSettings(section, ret) {
  if (typeof REMOTE !== "undefined" && REMOTE) {
    const p = {};
    if (section) p.section = section;
    if (ret) p.ret = "1";
    walkToSettings(p);
  } else {
    send({kind:"opensettings", section: section || null, ret: !!ret});
  }
}
// The phone's only way in: hand the token over once (the proxy trades it for a
// cookie and bounces to a URL without it), carrying which screen to land on.
function walkToSettings(params) {
  const q = new URLSearchParams(params).toString();
  location.href = "cfg?t=" + encodeURIComponent(TOKEN) + (q ? "&" + q : "");
}
// The tab bar's +. In the window this becomes Ctrl+B t, which opens the settings
// as a child WebView already adding a tab to the workspace in view. A phone has
// no such WebView and no keystroke that could summon one — the intent was simply
// refused from afar, so the + did nothing at all. It walks to the same page and
// asks for the same thing instead.
function addTabHere() {
  if (typeof REMOTE !== "undefined" && REMOTE) walkToSettings({addtab: (S && S.ws_index) || 0});
  else send({kind:"addtab"});
}
// Fetch the newest run's portable replay (durable anchors, no digest refs).
// The phone downloads it over HTTP; the window board has no HTTP downloads,
// so it asks the app to save the file into Downloads instead.
async function downloadReplayLua() {
  if (typeof REMOTE !== "undefined" && REMOTE) {
    try {
      const r = await fetch("api/replay?t=" + encodeURIComponent(TOKEN));
      if (!r.ok) { toast(T["tui.cast.replay.none"] || "No macro yet", true); return; }
      const blob = await r.blob();
      const u = URL.createObjectURL(blob);
      const a = document.createElement("a");
      a.href = u; a.download = "shikisha-macro.lua";
      document.body.append(a); a.click(); a.remove();
      setTimeout(() => URL.revokeObjectURL(u), 1000);
      // A silent success reads as a failure — say it landed
      toast(T["tui.cast.replay.downloaded"] || "Downloaded");
    } catch (e) {
      toast(String(e && e.message || e), true);
    }
  } else {
    // The app answers with the saved path (or the reason it couldn't) as a
    // message of its own; this one is the immediate "the press registered"
    send({kind:"replaysave"});
    toast(T["tui.cast.replay.saving"] || "Saving…");
  }
}
// The picker for the 🎯 panel: choose another tab to operate. Candidates are
// browsers and AI tabs only — the operate engine relays natural-language
// instructions, and typed into a plain shell/SSH/WSL those would execute as
// commands. The operator itself and INDEX are never candidates.
// Driving requires the operator (the active tab) to act WITHOUT confirmation — a
// model tab always does, a CLI only with its bypass flag. When it can't, the
// picker is disabled and a jump to settings is offered instead of a dead end.
function buildTargetPanel() {
  const wrap = el("div", {id:"casttarget"});
  const operator = (S && S.tabs) ? S.tabs.find(t => t && t.index === S.active) : null;
  const canOperate = !!(operator && operator.auto);
  // An aim outlives the page: it was written down against this tab when it was
  // picked, and S.aim is it, come back. Adopt it when this page has no aim of
  // its own yet, so a restart (or a phone opening the board) finds the 🎯 where
  // it was left rather than empty.
  if (!castTarget && S && S.aim) {
    const t = (S.tabs || []).find(x => x && x.index === S.aim);
    if (t) castTarget = { index: t.index, name: t.name || ("#" + t.index), kind: t.kind, model: !!t.model };
  }
  const tabs = (S && S.tabs)
    ? S.tabs.filter(t => t && t.index !== 0 && !t.settings && t.index !== S.active
        && (t.kind === "browser" || t.ai)) : [];
  const sel = el("select", {class:"castswitch"});
  sel.disabled = !canOperate;
  sel.append(el("option", {value:""}, T["tui.cast.target.none"] || "— none —"));
  tabs.forEach(t => {
    const icon = t.kind === "browser" ? "🌐" : (t.model ? "🤖" : "▷");
    const o = el("option", {value:String(t.index)}, icon + " " + (t.name || ("#" + t.index)));
    if (castTarget && castTarget.index === t.index) o.selected = true;
    sel.append(o);
  });
  sel.onchange = () => {
    if (!canOperate) { sel.value = ""; return; }
    const idx = parseInt(sel.value, 10);
    const t = tabs.find(x => x.index === idx);
    // Picking IS the setting: it is written down against this tab and comes
    // back on the next start. Aiming alone hands over no work — the operator
    // hears about it when a goal is sent — so this is safe to send on a
    // dropdown change.
    if (t) {
      castTarget = { index: t.index, name: t.name || ("#" + idx), kind: t.kind, model: !!t.model };
      send({kind:"operate", target: t.index});
    } else {
      send({kind:"operate", target: 0});
      castTarget = null;
    }
    renderPanel();   // the 🎯 chip appears/disappears with the choice
  };
  wrap.append(el("span", {class:"hint", style:"flex:none"}, T["tui.cast.target.label"] || "Operate:"), sel);
  // The finished operation's portable script, right where the target was
  // chosen. Always present: before any run exists, pressing it just says so
  wrap.append(el("button", {class:"castbtn", style:"flex:none", onclick: downloadReplayLua},
    T["tui.cast.target.replay"] || "⬇ Lua"));
  if (!canOperate) {
    castTarget = null;  // an unusable operator can't be aimed at anything
    wrap.append(
      el("span", {class:"hint", style:"flex:1 1 0;min-width:0;color:var(--danger)"},
        T["tui.cast.target.needauto"] || "This AI must be allowed to act without confirmation."),
      el("button", {class:"castbtn", style:"flex:none", onclick:openSettings},
        T["tui.cast.target.settings"] || "Settings"));
  } else {
    wrap.append(el("span", {class:"hint", style:"flex:1 1 0;min-width:0"}, T["tui.cast.target.hint"] || ""));
  }
  return wrap;
}
// Panels available on this surface. "target" (operate a tab) shows a placeholder
// until that feature lands, but it's listed now so the switcher is present on both
// the phone (keys/actions/target) and the desktop (actions/target).
function panelOptions() {
  const base = (typeof REMOTE !== "undefined" && REMOTE) ? ["keys", "actions"] : ["actions"];
  // A browser tab is operated, not an operator, so it has no 🎯 target panel —
  // instead it gains 📼 (record page actions as Lua / run composer Lua on the
  // page). Otherwise it's the same sub-input bar as an AI tab.
  if (onBrowserTab()) return base.concat("lua");
  const t = activeTab();
  // A model pane is a conversation, not a command line. There is nothing to
  // suggest a command into, and Send is the message itself — so it keeps the
  // base and nothing more: quick actions at the window, and the key row as
  // well on a phone.
  if (t && t.model) return base;
  // 🎯 exists ONLY where an AI can be the operator (the active tab drives
  // the chosen target). A plain terminal has no AI to drive anything, so it
  // gets no target panel at all — it gains 🤖 instead: natural language in,
  // one reviewed command out.
  if (t && t.ai) return base.concat("target");
  if (t && t.index !== 0 && !t.settings && t.kind !== "browser") {
    return base.concat("suggest");
  }
  return base;
}
// Window only: over a browser tab, reuse the composer (the sub-input bar) and
// reserve room on #page so the native browser — which is layered on top of the
// HTML — doesn't hide it. The composer FAB opens/closes it just like on an AI tab;
// the reserve follows that open/closed state (enough for the FAB, or for the dock).
// NOT rebuilt on every state push: replacing the panel's DOM kills a native
// <select> popup instantly, so while an AI tab streamed (state pushes every
// frame) the switcher's dropdown could never stay open. Rebuild only when the
// set of panels actually changed (entering/leaving a browser tab, first open).
let lastPanelSig = "";
function syncBrowserDock() {
  if (typeof REMOTE !== "undefined" && REMOTE) return;
  if (!onBrowserTab()) { lastPanelSig = ""; syncBrowserReserve(); return; }
  ensureBar();
  const sig = panelOptions().join();
  if (sig !== lastPanelSig) {
    lastPanelSig = sig;
    if (panelOptions().indexOf(castPanel) < 0) castPanel = panelOptions()[0];
    renderPanel();
  }
  syncBrowserReserve();
}
// Just the reserve: how much of #page the native browser must leave to the
// dock. Split out because the dock's height also changes when the composer
// textarea grows (a recorded line landing, a long paste) — that path needs the
// reserve refreshed without rebuilding the panel on every keystroke.
// Whether a page placed in the window should be drawing the pen: only when the
// composer is closed. WHICH page is the app's to work out -- it is the one in
// the focused pane -- so only this much travels. Sent on change, because it is
// a message and this runs on every frame
let lastPen = null;
function syncBrowserPen() {
  if (typeof REMOTE !== "undefined" && REMOTE) return;
  const want = !(castDock && castDock.style.display === "flex");
  if (want === lastPen) return;
  lastPen = want;
  send({kind:"pen", on: want});
}
function syncBrowserReserve() {
  if (typeof REMOTE !== "undefined" && REMOTE) return;
  syncBrowserPen();
  const page = document.getElementById("page");
  if (!onBrowserTab()) {
    if (page.style.getPropertyValue("--dock")) {
      page.style.removeProperty("--dock");
      scheduleReport();
    }
    return;
  }
  // Only the composer takes room. The pen used to take some too -- the page
  // was held up by the height of a button so that a button could be drawn
  // beside it, which left a band of nothing under every browser. The pen is
  // drawn by the page itself now and floats over it, so it costs nothing
  const dockOpen = castDock && castDock.style.display === "flex";
  const reserve = dockOpen ? Math.round(castDock.getBoundingClientRect().height) : 0;
  const want = reserve + "px";
  if (page.style.getPropertyValue("--dock") !== want) {
    page.style.setProperty("--dock", want);
    scheduleReport();
  }
}
// Full name (for the switcher's hover title / accessibility).
function panelName(p) {
  return p === "keys" ? (T["tui.cast.panel.keys"] || "Keys")
    : p === "actions" ? (T["tui.cast.panel.actions"] || "Actions")
    : p === "lua" ? (T["tui.cast.panel.lua"] || "Lua record / run")
    : p === "suggest" ? (T["tui.cast.panel.suggest"] || "AI command suggest")
    : (T["tui.cast.panel.target"] || "Target");
}
// A compact emoji for the switcher itself — text labels ate horizontal width.
function panelLabel(p) {
  return p === "keys" ? "⌨️" : p === "actions" ? "⚡" : p === "lua" ? "📼"
    : p === "suggest" ? "🤖" : "🎯";
}
function panelContent(p) {
  if (p === "keys") { castKeysEl = buildCastKeys(); return castKeysEl; }
  if (p === "actions") { return buildActions() || el("div", {class:"castpanelhint"}, T["settings.actions.empty"] || ""); }
  if (p === "target") { return buildTargetPanel(); }
  if (p === "lua") { return buildLuaPanel(); }
  if (p === "suggest") { return buildSuggestPanel(); }
  return null;
}
// The ✨ panel: natural language in, ONE command out — drafted into the
// composer for the person to review and Send. Nothing runs on its own.
let suggestBusy = false, suggestDraft = "";
function buildSuggestPanel() {
  const wrap = el("div", {id:"castsuggest", style:"display:flex;flex:1 1 0;gap:8px;align-items:center;min-width:0;padding:6px 0"});
  const inp = el("input", {type:"text", style:"flex:1 1 0;min-width:120px;padding:6px 10px;font-size:13px;background:var(--bg);color:var(--text);border:1px solid var(--line);border-radius:8px",
    placeholder: T["tui.cast.suggest.ph"] || "What do you want to do?"});
  inp.value = suggestDraft;
  inp.addEventListener("input", () => { suggestDraft = inp.value; });
  inp.addEventListener("keydown", (e) => { if (e.key === "Enter") { e.preventDefault(); go(); } });
  const btn = el("button", {class:"castbtn", style:"flex:none", onclick: go},
    suggestBusy ? "…" : ("🤖 " + (T["tui.cast.suggest.go"] || "Suggest")));
  function go() {
    const t = inp.value.trim();
    if (!t || suggestBusy) return;
    suggestBusy = true;
    renderPanel();
    send({kind:"suggest", text: t});
  }
  // 🩺 the environment survey (the "doctor" idiom — flutter doctor, brew
  // doctor): DRAFTS a fixed read-only probe into the composer for the person
  // to review and send — even a canned probe never types itself. Its output
  // becomes the tab's environment card, carried into every ✨ suggestion.
  // For bastion hops, press it again after landing on the new host
  const doc = el("button", {class:"castbtn", style:"flex:none",
    title: T["tui.cast.survey.hint"] || "Survey this terminal's environment",
    onclick: () => { send({kind:"survey"}); }},
    "🩺 " + (T["tui.cast.survey.go"] || "Survey"));
  wrap.append(inp, btn, doc);
  return wrap;
}
// 🩺 progress from the loop (or the phone's state push)
window.__surveyed = (r) => {
  if (r && r.stage === "draft" && r.cmd) {
    ensureBar();
    if (castDock && castDock.style.display !== "flex") openTermBar();
    castSlot = "draft";
    castDraft = r.cmd;
    if (castInput) { castInput.value = r.cmd; growCastInput(); }
    toast(T["tui.cast.survey.drafted"] || "🩺 Review the survey command, then Send");
  } else if (r && r.ok) {
    toast(T["tui.cast.survey.done"] || "🩺 Environment recorded — suggestions now use it");
  } else {
    toast((r && r.error) || (T["tui.cast.survey.failed"] || "Survey failed"), true);
  }
};
// The finished suggestion (or its failure) — from the window loop or the
// phone's state push. The command lands in the composer as a DRAFT: the
// person reads it and presses Send themselves
window.__suggested = (r) => {
  suggestBusy = false;
  if (r && r.ok && r.cmd) {
    ensureBar();
    if (castDock && castDock.style.display !== "flex") openTermBar();
    castSlot = "draft";
    castDraft = r.cmd;
    if (castInput) { castInput.value = r.cmd; growCastInput(); }
    toast(T["tui.cast.suggest.ready"] || "✨ Review the command, then Send");
  } else {
    toast((r && r.error) || (T["tui.cast.suggest.failed"] || "Suggestion failed"), true);
  }
  renderPanel();
};
// The 📼 panel: ⏺ turns what happens on the shown page into Lua lines in the
// composer (Send becomes Copy); ▶ runs the composer's Lua against the page in
// the same sandbox the rally's AI code runs in (Send becomes Run). Recording
// is armed server-side and survives panel switches (the phone needs ⌨️ while
// recording); a TAB switch turns it off — see the __state handler.
// 📼's transient status, shown in the panel's hint slot until the next action.
// A toast can't do this job: over a browser tab the native page is layered on
// top of the HTML, so anything outside the dock's reserved band is invisible.
let luaNote = null;
function luaFlash(text, bad) { luaNote = {text: text, bad: !!bad}; if (castPanel === "lua") renderPanel(); }
function buildLuaPanel() {
  const wrap = el("div", {id:"castlua"});
  const mk = (mode, glyph, label) => {
    const lab = el("label", {class:"castradio"});
    const r = el("input", {type:"radio", name:"luamode", value:mode});
    if (luaMode === mode) r.checked = true;
    r.onchange = () => {
      luaMode = mode;
      luaNote = null;
      send({kind:"record", on: mode === "rec"});
      renderPanel();
    };
    lab.append(r, document.createTextNode(glyph + " " + label));
    return lab;
  };
  const hint = luaNote ? luaNote.text
    : luaMode === "rec"
      ? (T["tui.cast.lua.rechint"] || "Type and tap as usual — every step is recorded. ▶ shows the Lua.")
      : (T["tui.cast.lua.runhint"] || "The recorded Lua — edit it, Run it, 📋 copies it.");
  const tone = luaNote ? (luaNote.bad ? ";color:var(--danger)" : ";color:var(--brand)") : "";
  wrap.append(
    mk("rec", "⏺", T["tui.cast.lua.rec"] || "Record"),
    mk("run", "▶", T["tui.cast.lua.run"] || "Run"),
    el("button", {class:"castbtn", style:"flex:none",
      title: T["tui.cast.lua.copy"] || "Copy the recorded Lua",
      onclick: copySheet}, "📋"),
    el("span", {class:"hint", style:"flex:1 1 0;min-width:0" + tone, title: hint}, hint));
  return wrap;
}
// 📋: the Lua sheet to the clipboard (whether or not it's currently loaded in
// the composer). navigator.clipboard needs a secure context, which the phone
// (plain http) doesn't have — fall back to a temporary selection there.
function copySheet() {
  const text = castSlot === "lua" && castInput ? castInput.value : luaSheet;
  if (!text) return;
  const done = () => luaFlash("📋 " + (T["tui.cast.lua.copied"] || "Copied"));
  const fallback = () => {
    try {
      const tmp = el("textarea", {style:"position:fixed;left:-9999px;top:0"});
      tmp.value = text;
      document.body.append(tmp);
      tmp.select();
      document.execCommand("copy");
      tmp.remove();
      done();
    } catch (e) {}
  };
  if (navigator.clipboard && navigator.clipboard.writeText) {
    navigator.clipboard.writeText(text).then(done, fallback);
  } else { fallback(); }
}
// A recorded step arrived (window: eval'd in; phone: over /ws-state). It goes
// on the Lua sheet (shown in ▶ run mode); while ⏺ recording, the panel's hint
// echoes the newest line so there's live proof the recorder is listening.
window.__recorded = function (line) {
  try {
    ensureBar();
    if (castSlot === "lua") {
      const cur = castInput.value;
      castInput.value = cur + (cur && !cur.endsWith("\n") ? "\n" : "") + line + "\n";
      luaSheet = castInput.value;
      growCastInput();
    } else {
      luaSheet += (luaSheet && !luaSheet.endsWith("\n") ? "\n" : "") + line + "\n";
      if (castPanel === "lua") luaFlash("⏺ " + line);
    }
  } catch (e) {}
};
// The verdict of a ▶ run: null = ran clean, a string = the error text.
window.__luaDone = function (err) {
  try {
    luaFlash(err ? ("⚠ " + String(err).slice(0, 300)) : ("✔ " + (T["tui.cast.lua.ok"] || "Ran")), !!err);
  } catch (e) {}
};
// 📎 attaches a file FOR AN AI — it's saved and its path dropped into the
// composer to hand over. While the composer feeds a browser page (window native
// or phone relay) a path is just text typed into the site, so the button hides.
function syncAttach() { if (castAttEl) castAttEl.style.display = drivingBrowser() ? "none" : ""; }
// Load the composer with the document the active panel owns (see the luaSheet
// comment): 📼 shows the Lua sheet, every other panel the ordinary draft.
// Edits made while a slot is loaded are stashed when switching away.
function syncComposerSlot() {
  if (!castInput) return;
  const want = (castPanel === "lua" && luaMode === "run") ? "lua" : "draft";
  // The prompt is re-asked every render, not only when the document changes:
  // walking from a terminal to a model pane swaps where a Send goes without
  // swapping the document, and a field that still said "type here to send"
  // would be describing the tab we just left.
  castInput.placeholder = want === "lua"
    ? (T["tui.cast.lua.ph"] || "Recorded Lua appears here — edit, Run, or write your own")
    : onModelTab()
      ? (T["tui.chat.ph"] || "Message {model}\u2026").split("{model}").join((activeTab() || {}).name || "model")
      : (T["tui.cast.type.ph"] || "Type here to send");
  if (want === castSlot) return;
  if (castSlot === "lua") { luaSheet = castInput.value; } else { castDraft = castInput.value; }
  castInput.value = want === "lua" ? luaSheet : castDraft;
  castSlot = want;
  growCastInput();
}
// The Send button doubles as 📼's verb: Copy while recording (the recorded Lua
// is a deliverable, not something to send), Run in run mode, plain Send elsewhere.
function syncSendLabel() {
  if (!castSendEl) return;
  // Only ▶ run mode takes the button over; ⏺ recording leaves Send as the
  // ordinary page/terminal input (that input is exactly what gets recorded).
  castSendEl.textContent =
    castPanel === "lua" && luaMode === "run"
      ? (T["tui.cast.lua.exec"] || "Run")
      : (T["tui.cast.send"] || "Send");
}
// (Re)fill the panel area: the fixed switcher (only when there's more than one
// choice) plus the current panel's content. The switcher sits outside the
// scrolling content, so it stays put while the chips/keys scroll under it.
function renderPanel() {
  if (!castPanelEl) return;
  syncAttach();
  const opts = panelOptions();
  // The panel only changes when the PERSON changes it. Tab switches (the
  // rally flipping between the AI and the browser) can make the chosen
  // panel temporarily unavailable — fall back for the render, but keep the
  // choice and restore it the moment it's available again.
  if (userPanel && opts.indexOf(userPanel) >= 0) castPanel = userPanel;
  else if (opts.indexOf(castPanel) < 0) castPanel = opts[0];
  castPanelEl.textContent = "";
  if (opts.length > 1) {
    const sel = el("select", {class:"castswitch", title: panelName(castPanel)});
    // Emoji-only options: a native select shows the selected option's text when
    // collapsed, so the label has to BE the emoji to stay narrow. The full name
    // rides along as each option's title for hover / accessibility.
    opts.forEach(p => { const o = el("option", {value:p, title: panelName(p)}, panelLabel(p)); if (p === castPanel) o.selected = true; sel.append(o); });
    sel.onchange = () => { castPanel = sel.value; userPanel = sel.value; renderPanel(); };
    castPanelEl.append(sel);
  }
  // While a 🎯 target is aimed, every panel says so — the person switching
  // panels otherwise assumes the targeting ended, then their next Send goes
  // to the operate goal instead of the terminal. ✕ releases the target
  if (castTarget) {
    const chip = el("span", {class:"castchip",
      title: T["tui.cast.target.active_hint"] || "Send goes to the AI as a goal for this tab"});
    chip.append(document.createTextNode("🎯 " + (castTarget.name || "")));
    chip.append(el("button", {class:"castchipx",
      title: T["tui.cast.target.clear"] || "Release",
      onclick: () => {
        send({kind:"operate", target: 0});
        castTarget = null;
        toast(T["tui.cast.target.cleared"] || "🎯 released");
        renderPanel();
      }}, "✕"));
    castPanelEl.append(chip);
  }
  const content = panelContent(castPanel);
  if (content) castPanelEl.append(content);
  // A fixed ⚙ at the right edge of the actions panel jumps straight to the Quick
  // Actions settings card (and returns here once saved). Stays put while the chips
  // scroll under it, mirroring the switcher on the left.
  if (castPanel === "actions") {
    castPanelEl.append(el("button", {class:"castgear",
      title: T["tui.cast.actions.edit"] || "Edit quick actions",
      onclick: () => openSettings("actions", true)}, "⚙️"));
  }
  // After the reset above, castPanel is final for this render — load the
  // panel's composer document and relabel Send to its verb.
  syncComposerSlot();
  syncSendLabel();
}
function ensureBar() {
  if (castDock) return;
  // A textarea (not <input>) so it can hold newlines: Shift+Enter inserts one and
  // multi-line text pastes intact. It starts one row tall and grows with the text.
  // Everything the phone keyboard likes to "help" with is turned off: what goes
  // in here is a command or a password, and a corrected one is a wrong one
  castInput = el("textarea", {id:"castinput", rows:"1", autocomplete:"off",
    autocapitalize:"off", autocorrect:"off", spellcheck:"false",
    placeholder: T["tui.cast.type.ph"] || "Type here to send"});
  castSendEl = el("button", {class:"castsend", onclick:sendBar}, T["tui.cast.send"] || "Send");
  const bs = el("button", {class:"castbtn", onclick:() => sendCastKey("backspace")}, "⌫");
  // ✕ only dismisses the keyboard (the sub-input bar itself stays visible throughout control mode)
  // In browser control mode ✕ only drops the keyboard (the bar stays — the relay
  // cursor is still in control). Over a terminal it closes the bar entirely, back
  // to a full-screen reading view.
  const close = el("button", {class:"castbtn", onclick:() => {
    if (castInput) castInput.blur();
    // Over a terminal this ✕ is the person saying "out of my way" (vi and
    // friends) — remember it, so the bar stays collapsed on this machine
    // until the ✎ pen summons it again
    if (!castMode) { closeBar(); rememberCastClosed(true); }
  }}, "✕");
  // Attach a file: pick or paste an image / PDF. It's saved beside the target
  // tab (under .SHIKISHA/tmp) and its path is dropped into the composer to hand
  // to the AI — a CLI can't take an attachment, you give it a path.
  const fileIn = el("input", {type:"file", accept:"image/*,application/pdf",
    style:"display:none",
    onchange:(e) => { const f = e.target.files && e.target.files[0]; if (f) attachFile(f); e.target.value = ""; }});
  castAttEl = el("button", {class:"castbtn", title: T["tui.cast.attach"] || "Attach a file",
    onclick:() => fileIn.click()}, "📎");
  // ⌫ and Send keep the input field's focus (= the keyboard) in place. If
  // the default pointerdown action weren't prevented, focus would shift to
  // the button, the keyboard would close, and typing couldn't continue.
  // ✕ is deliberately excluded since closing it is the whole point
  [bs, castSendEl].forEach(b => b.addEventListener("pointerdown", (e) => e.preventDefault()));
  // Attach works on both now (phone over HTTP, window over ipc). The backspace
  // key is only useful on the phone, whose on-screen keyboard the composer
  // sometimes covers; the window has a real keyboard. el() skips nulls.
  castBar = el("div", {id:"castbar"}, castAttEl, fileIn, (REMOTE ? bs : null), castInput, castSendEl, close);
  // One switchable panel above the input row (keys / actions / target), chosen by
  // a fixed switcher, instead of stacking every row at once. Default: keys on the
  // phone, actions on the desktop.
  castPanel = panelOptions()[0];
  castPanelEl = el("div", {id:"castpanel"});
  renderPanel();
  // The release banner (browser-relay control mode only). Shifts up with the dock
  // once the keyboard appears.
  modeEl = el("div", {id:"castmode"}, el("span", {}, T["tui.cast.control"] || "In control — tap to release"));
  modeEl.onclick = exitCast;
  // Rows top-to-bottom: release banner, the switchable panel, the input row.
  castDock = el("div", {id:"castdock"}, modeEl, castPanelEl, castBar);
  document.getElementById("main").append(castDock);
  // Enter sends; Shift+Enter (or an active IME) inserts a newline instead.
  castInput.addEventListener("keydown", (e) => {
    if (e.isComposing) return;
    if (e.key === "Enter" && !e.shiftKey) { sendBar(); e.preventDefault(); }
  });
  // Grow the field with its content (up to the CSS max-height, then it scrolls).
  castInput.addEventListener("input", growCastInput);
  // Paste an image straight into the composer — it's saved and its path inserted.
  // Not while feeding a browser (same reason the 📎 button hides there).
  castInput.addEventListener("paste", (e) => {
    if (drivingBrowser()) return;
    const fs = e.clipboardData && e.clipboardData.files;
    if (fs && fs.length) { attachFile(fs[0]); e.preventDefault(); }
  });
  // Lift the dock by the keyboard's height (so it doesn't hide underneath it).
  // As its own term, added to the pane's offset by the CSS above -- never as
  // the whole of `bottom`, which would throw the pane away
  if (window.visualViewport) {
    const fit = () => {
      const gap = window.innerHeight - window.visualViewport.height - window.visualViewport.offsetTop;
      castDock.style.setProperty("--kbd", Math.max(0, gap) + "px");
    };
    window.visualViewport.addEventListener("resize", fit);
    window.visualViewport.addEventListener("scroll", fit);
  }
}
// Whether the ✏️ pen — the way back into a collapsed composer — is showing.
//
// Two things decide it: the composer must be closed (open, the pen would sit on
// top of the bar's own ✕), and the place we are in must have something to
// summon. The second half changes WITHOUT the bar being touched — the person
// walks to another tab — so it is settled here, from the state, on every
// update. Deciding it inside closeBar() froze the answer at the moment of
// closing: shut the bar over a placed page (which draws its own pen, so the
// window's is hidden) and the pen never came back on the next tab, because
// nothing ever asked the question again.
//
// This is the app's own pen. The one a placed page draws for itself is
// syncBrowserPen's business — same rule, different surface.
function syncPen() {
  if (!fab) return;
  const open = castDock && castDock.style.display === "flex";
  // A phone shows it only over a real terminal tab: a browser relay and a model
  // chat carry their own composer, so summoning the terminal bar there would be
  // nonsense. At the window the exception is the placed page — the window's pen
  // would be drawn underneath it and never seen, so that page draws one itself.
  const here = !covering()
    && ((typeof REMOTE !== "undefined" && REMOTE) ? onTermPty() : !onBrowserTab());
  fab.style.display = (!open && here) ? "" : "none";
}
// Show the sub-input bar (auxiliary key row + input field). Never focused
// automatically — the keyboard never pops up uninvited. It only opens when the user taps the input field
function showDock() { ensureBar(); syncAttach(); castDock.style.display = "flex"; syncPen(); }
function closeBar() {
  if (castDock) castDock.style.display = "none";
  if (castInput) castInput.blur();
  syncPen();
}
// Hand one line to a pane, the way THAT pane takes input: a message for a
// model bridge, keystrokes and a submit for anything at a prompt. Everywhere a
// person finishes a line comes through here -- the composer's Send and the
// discussion's topic box -- so what pressing Enter means cannot come to mean
// two different things in two places. `tab` names the recipient when it is not
// the pane in front (the topic box aims at the opening speaker); the caller
// puts it in front first, exactly as a person would.
//
// A model pane has no command line, so an empty line has nothing to accept and
// does nothing; at a prompt a bare Enter is itself the instruction (accept a
// default, insert a newline), so it is still sent.
function sendLine(text, tab) {
  // Say who it is for. Left unsaid, a line is handed to whatever pane happens
  // to be in front when it arrives -- the same pane nearly always, which is why
  // this went unnoticed until the topic box, which puts the opening speaker in
  // front and hands over the topic in the same breath. Those are two messages,
  // and nothing promises they land in that order: the topic reached the pane
  // being looked at, or, if that pane could not take it, nobody at all.
  if (text) { send({kind:"say", tab: (tab == null ? S.active : tab), text}); return; }
  // An empty Send is a bare Enter: meaningful at a prompt (accept a default,
  // insert a newline) and not a line at all, so it stays a keystroke.
  send({kind:"key", named:"enter"});
}
function sendBar() {
  if (!castInput) return;
  const t = castInput.value;
  // 📼's ▶ run mode owns the button: Run the sheet on the shown page. The
  // text stays put — it's a document being iterated, not a message. In ⏺
  // record mode the button is NOT taken over: Send keeps feeding the page
  // (and that injection is what the recorder captures).
  if (castPanel === "lua" && luaMode === "run") {
    if (t) { luaNote = null; renderPanel(); send({kind:"runlua", code: t}); }
    return;
  }
  if (drivingBrowser()) {
    // Browser (phone relay or window native): inject the confirmed text as one
    // batch into the shown page, or a bare Enter. Same path for both surfaces.
    if (t) injectIn({kind:"inject", what:"text", text:t});
    else sendCastKey("enter");
  } else if (castTarget) {
    // 🎯 operate mode: hand the text to the active AI as a goal, and it drives the
    // chosen target (writes Lua). Not typed into the terminal we're viewing.
    send({kind:"operate", target: castTarget.index, goal: t});
    // A bare tab name reads as noise — say what actually happened to the text
    toast((T["tui.cast.target.sent"] || "🎯 Asked the AI to drive {name}")
      .replace("{name}", castTarget.name || ""));
  } else if (modCtrl && t) {
    // Terminal: Ctrl latched + a typed letter = a control chord (e.g. Ctrl+C to
    // interrupt). Takes the first character; no trailing Enter — a chord isn't a line.
    send({kind:"key", ctrl: t.slice(0, 1).toLowerCase()});
    modCtrl = false; modAlt = false; refreshMods();
  } else {
    // Hand the line to the pane in front, whatever kind of pane it is.
    sendLine(t);
  }
  castInput.value = "";
  castInput.focus();
  growCastInput();
}
// The cursor is absolutely positioned within #main. Compute #cast's content
// position relative to #main via castRect() — getBoundingClientRect() already
// reflects the pinch-zoom transform, so the arrow stays glued to the page
function posCursor() {
  const cv = document.getElementById("cast");
  const m = document.getElementById("main").getBoundingClientRect();
  const c = castRect(cv);
  cursorEl.style.left = (c.ox - m.left + cx * c.dw) + "px";
  cursorEl.style.top = (c.oy - m.top + cy * c.dh) + "px";
}
// Once control mode is entered, keep the sub-input bar shown at all times
// (so the auxiliary keys work without needing to press a button first)
function enterCast() { ensureCursor(); castMode = true; if (modeEl) modeEl.style.display = ""; cursorEl.style.display = "block"; showDock(); posCursor(); }
// Open the sub-input bar over a phone terminal tab. No relay cursor and no "in
// control" banner — just the auxiliary keys and the text field. Like the browser
// bar, the keyboard only opens once the user taps the field itself, so a stray
// tap never throws the keyboard up over the screen.
function openTermBar() {
  castMode = false;
  showDock();
  if (modeEl) modeEl.style.display = "none";
  modCtrl = false; modAlt = false; refreshMods();
}
function exitCast() { castMode = false; dragging = false; if (cursorEl) cursorEl.style.display = "none"; closeBar(); }
// The desktop window's summonable composer. Direct typing into the terminal
// stays the default (the hidden #kbd); this bar is opened on demand for editing
// a longer instruction, the quick actions, and (later) attachments. sendBar()
// posts the very same {kind:"key",…} intents #kbd does, so it reaches the active
// tab's PTY with no extra wiring.
// The person's "keep the composer out of my way" choice, per machine
function rememberCastClosed(v) {
  try { localStorage.setItem("shikishaCastClosed2", v ? "1" : ""); } catch (e) {}
}
// ...and the one place that reads it back. Every path that might open the bar on
// its own asks here first, so the meaning of the ✕ can't differ between them
function castClosed() {
  try { return localStorage.getItem("shikishaCastClosed2") === "1"; } catch (e) { return false; }
}
// The pen, pressed on a page placed in the window. Its press arrives as a
// message rather than a click, because that page is a window of its own -- but
// it is the same pen and goes through the same one door, so what the ✕ means
// cannot start drifting between two ways in. That page only draws a pen while
// the composer is shut, so the toggle can only ever open
window.__composer = function () { toggleComposer(); };
function toggleComposer() {
  ensureBar();
  // showDock sets "flex", closeBar sets "none"; a fresh dock has "" (CSS hides it),
  // so test for the shown value rather than "!== none" (which a blank string passes).
  const showing = castDock && castDock.style.display === "flex";
  if (showing) { closeBar(); rememberCastClosed(true); }
  else {
    openTermBar();
    if (castInput) castInput.focus();
    rememberCastClosed(false);
  }
  // Over a browser tab, the reserved room on #page follows the open/closed state.
  syncBrowserDock();
}
let fab = null;
// Both surfaces get the ✏️ pen: with the composer collapsed there must be a
// VISIBLE way back in (on the phone, "tap the terminal" also works, but an
// invisible affordance is no affordance). It hides while the bar is open
fab = el("button", {id:"composerfab", title: T["tui.cast.compose"] || "Composer",
  onclick: toggleComposer}, "✏️");
document.getElementById("main").append(fab);
if (!REMOTE) {
  // The composer is the workbench, not a popup: shown by default. Only the
  // person's own ✕ (recorded above) keeps it collapsed behind the pen.
  // Opened without stealing focus — direct typing still goes to the terminal
  if (!castClosed()) {
    ensureBar();
    openTermBar();
    if (castInput) castInput.blur();
    syncBrowserDock();
  }
}
// The element directly under the arrow's tip. The synthetic arrow and
// ripple both use pointer-events:none, so they're transparent to hit
// testing, and the real thing beneath them (a bar button/URL field, or the relay canvas) is returned instead
function underCursor() {
  if (!cursorEl) return null;
  const m = document.getElementById("main").getBoundingClientRect();
  const x = m.left + (parseFloat(cursorEl.style.left) || 0);
  const y = m.top + (parseFloat(cursorEl.style.top) || 0);
  return document.elementFromPoint(x, y);
}
const click = () => {
  // If the arrow is over the app's own bar (back/forward/reload/URL),
  // don't inject into the browser — operate that UI directly instead.
  // This gives the cursor the same behavior a direct tap would have
  const hit = underCursor();
  if (hit && hit.closest && hit.closest("#nav")) {
    const b = hit.closest("button");
    if (b) { b.click(); spawnRipple(); return; }
    const inp = hit.closest("input");
    if (inp) { inp.focus(); if (inp.select) inp.select(); spawnRipple(); return; }
    return;   // empty space in the bar — do nothing, so the page below isn't tapped by accident
  }
  sendIn({kind:"inject", what:"mouse", phase:"pressed",  x:cx, y:cy, down:true});
  sendIn({kind:"inject", what:"mouse", phase:"released", x:cx, y:cy, down:false});
  spawnRipple();
};
function bindCastInput(cv) {
  if (castBound) return; castBound = true;
  const pts = new Map(); let lastTapT = 0, moved = false, startT = 0;
  // A two-finger gesture starts undecided ("?") and commits to one meaning:
  // fingers moving apart/together = pinch zoom; sliding in parallel = pan the
  // zoomed view when magnified, otherwise scroll the page (the old behavior)
  let gest = null, gd = 0, gmx = 0, gmy = 0;
  const two = () => {
    const a = [...pts.values()];
    return { d: Math.hypot(a[0].x - a[1].x, a[0].y - a[1].y),
             mx: (a[0].x + a[1].x) / 2, my: (a[0].y + a[1].y) / 2 };
  };
  cv.addEventListener("pointerdown", (e) => {
    pts.set(e.pointerId, {x: e.clientX, y: e.clientY});
    try { cv.setPointerCapture(e.pointerId); } catch (x) {}
    e.preventDefault();
    if (pts.size === 2) { const t = two(); gest = "?"; gd = t.d; gmx = t.mx; gmy = t.my; }
    if (!castMode) { enterCast(); return; }   // the very first tap only enters control mode
    if (pts.size >= 2) return;                 // two fingers: zoom / pan / scroll
    startT = Date.now(); moved = false;
    if (Date.now() - lastTapT < 300) {         // tap-then-drag = grab
      dragging = true;
      sendIn({kind:"inject", what:"mouse", phase:"pressed", x:cx, y:cy, down:true});
    }
  });
  cv.addEventListener("pointermove", (e) => {
    if (!castMode) return; e.preventDefault();
    const p = pts.get(e.pointerId);
    if (p) { p.x = e.clientX; p.y = e.clientY; }
    if (pts.size >= 2) {
      const t = two();
      if (gest === "?") {  // undecided: commit once the fingers clearly do one or the other
        if (Math.abs(t.d - gd) > 14) gest = "zoom";
        else if (Math.hypot(t.mx - gmx, t.my - gmy) > 14) gest = (zs > 1.001) ? "pan" : "scroll";
        else return;
      }
      if (gest === "zoom") {
        // Scale about the fingers' midpoint: the content point under it before
        // the change must sit under it after (the standard pinch feel)
        const ns = Math.min(5, Math.max(1, zs * (t.d / (gd || t.d))));
        const rr = cv.getBoundingClientRect();
        const ex = rr.left - zx, ey = rr.top - zy;   // the canvas's untransformed origin
        const u = (gmx - ex - zx) / zs, v = (gmy - ey - zy) / zs;
        zx = t.mx - ex - u * ns;
        zy = t.my - ey - v * ns;
        zs = ns;
        applyZoom();
        posCursor();
      } else if (gest === "pan") {
        zx += t.mx - gmx; zy += t.my - gmy;
        applyZoom();
        posCursor();
      } else {                                  // "scroll": vertical movement becomes wheel scroll
        const dy = t.my - gmy;
        if (dy) sendIn({kind:"inject", what:"wheel", x:cx, y:cy, dx:0, dy:-dy * 6});
      }
      gd = t.d; gmx = t.mx; gmy = t.my;
      return;
    }
    const mx = e.movementX || 0, my = e.movementY || 0;
    if (Math.abs(mx) + Math.abs(my) > 2) moved = true;
    const r = castRect(cv);
    cx = clamp01(cx + mx * CURSOR_ACCEL / r.dw);
    cy = clamp01(cy + my * CURSOR_ACCEL / r.dh);
    posCursor();
    sendIn({kind:"inject", what:"mouse", phase:"moved", x:cx, y:cy, down:dragging});
  });
  const up = (e) => {
    pts.delete(e.pointerId);
    if (pts.size < 2) gest = null;
    if (!castMode) return; e.preventDefault();
    if (pts.size >= 1) return;                  // another finger is still down
    if (dragging) { sendIn({kind:"inject", what:"mouse", phase:"released", x:cx, y:cy, down:false}); dragging = false; return; }
    if (!moved && Date.now() - startT < 300) { click(); lastTapT = Date.now(); }
  };
  cv.addEventListener("pointerup", up);
  cv.addEventListener("pointercancel", up);
  // Also forward the mouse wheel, for testing in a desktop browser
  cv.addEventListener("wheel", (e) => {
    if (!castMode) return;
    sendIn({kind:"inject", what:"wheel", x:cx, y:cy, dx:e.deltaX, dy:e.deltaY});
    e.preventDefault();
  }, {passive:false});
}

send({kind:"ready"});
</script></body></html>"####;

// ── Terminal contents ────────────────────────────
// This is the one place that's fine staying a grid of cells — it really is
// a grid. The shell (tab bar, dashboard) is written as real HTML.

/// Converts a color index into a CSS color.
///
/// 0-15 are the sixteen a theme names, and are written as the variables the
/// page defines rather than as colours: this is the whole of what a colour
/// scheme has to reach, and rendering a screen stays a thing that needs to
/// know nothing about themes. 16-231 are a 6x6x6 color cube and 232-255 are a
/// grayscale ramp -- those are arithmetic, a terminal convention, and are not
/// a scheme's to redefine.
fn color_css(c: vt100::Color, fallback: &'static str) -> String {
    match c {
        vt100::Color::Default => fallback.to_string(),
        vt100::Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        vt100::Color::Idx(i) => match i {
            0..=15 => format!("var(--c{i})"),
            16..=231 => {
                let i = i - 16;
                let step = |v: u8| if v == 0 { 0u8 } else { 55 + v * 40 };
                format!(
                    "#{:02x}{:02x}{:02x}",
                    step(i / 36),
                    step((i / 6) % 6),
                    step(i % 6)
                )
            }
            _ => {
                let v = 8 + (i - 232) * 10;
                format!("#{v:02x}{v:02x}{v:02x}")
            }
        },
    }
}

fn esc_into(out: &mut String, s: &str) {
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(ch),
        }
    }
}

/// Renders the screen as HTML, one string per row.
///
/// Rows are kept apart because that is the shape of a change: a spinner
/// turning over redraws one line, and handing the page a whole new grid for
/// it makes it throw every element on screen away and build it again. That
/// cost lands on whoever is typing at that moment -- the browser has to
/// finish the new layout before it can answer "how tall is this box now?".
///
/// Consecutive cells with the same appearance are merged into a single
/// span. Making an element per cell would produce 9000 elements for a
/// 50-row by 180-column screen, making every frame's redraw expensive.
pub fn screen_rows(screen: &vt100::Screen) -> Vec<String> {
    // The two the screen already has: text that was given no colour is the
    // window's text colour, and a cell with no background shows the window
    const FG: &str = "var(--text)";
    const BG: &str = "transparent";
    let (rows, cols) = screen.size();
    let mut out_rows: Vec<String> = Vec::with_capacity(rows as usize);
    // How the cell before this one looked, and the CSS it came out as. A
    // screen is overwhelmingly cells that look like their neighbour, so
    // spelling the same declaration out again for each of them is thousands
    // of throwaway strings per frame, for a picture that is usually identical
    let mut seen: Option<Look> = None;
    let mut seen_style = String::new();
    for r in 0..rows {
        let mut out = String::with_capacity(cols as usize * 2);
        let mut open: Option<String> = None;
        let mut run = String::new();
        // How many cells this run spans. Position is determined by this, not by font advance width
        let mut span = 0usize;
        for c in 0..cols {
            let Some(cell) = screen.cell(r, c) else { continue };
            if cell.is_wide_continuation() {
                continue;
            }
            let look = Look {
                fg: cell.fgcolor(),
                bg: cell.bgcolor(),
                inverse: cell.inverse(),
                bold: cell.bold(),
                dim: cell.dim(),
                italic: cell.italic(),
                underline: cell.underline(),
            };
            if seen != Some(look) {
                seen = Some(look);
                seen_style.clear();
                let (mut fg, mut bg) = (look.fg, look.bg);
                if look.inverse {
                    std::mem::swap(&mut fg, &mut bg);
                }
                let fgc = color_css(fg, if look.inverse { BG } else { FG });
                if fgc != FG {
                    seen_style.push_str("color:");
                    seen_style.push_str(&fgc);
                    seen_style.push(';');
                }
                let bgc = color_css(bg, if look.inverse { FG } else { BG });
                if bgc != BG {
                    seen_style.push_str("background:");
                    seen_style.push_str(&bgc);
                    seen_style.push(';');
                }
                if look.bold {
                    seen_style.push_str("font-weight:700;");
                }
                if look.dim {
                    seen_style.push_str("opacity:.6;");
                }
                if look.italic {
                    seen_style.push_str("font-style:italic;");
                }
                if look.underline {
                    seen_style.push_str("text-decoration:underline;");
                }
            }
            let style = seen_style.as_str();
            // Break the run wherever the appearance changes
            if open.as_deref() != Some(style) {
                if let Some(prev) = open.take() {
                    flush_run(&mut out, &prev, &run, span);
                    run.clear();
                    span = 0;
                }
                open = Some(style.to_string());
            }
            let ch = cell.contents();
            let wide = cell.is_wide();
            // Only plain ASCII gets merged into a run.
            //
            // The cell width is derived from measuring ASCII characters, so
            // ASCII always fits its box exactly. Anything else may come
            // from a different font, whose advance width doesn't match the
            // cell. CJK characters render narrower than 2 cells each, so
            // merging them lets the shortfall accumulate at the end of the
            // run — after just 40 characters, a gap of about 10 cells had
            // opened up between the end of the string and the cursor.
            // Boxing each character individually instead spreads the
            // shortfall evenly between characters, so it never accumulates anywhere.
            if !wide && (ch.is_empty() || ch.chars().all(|c| c.is_ascii())) {
                span += 1;
                if ch.is_empty() {
                    run.push(' ');
                } else {
                    esc_into(&mut run, ch);
                }
                continue;
            }
            // Anything that can't be merged gets its own single-character box
            flush_run(&mut out, style, &run, span);
            run.clear();
            span = 0;
            let mut one = String::new();
            esc_into(&mut one, ch);
            flush_cell(&mut out, style, &one, if wide { 2 } else { 1 });
        }
        if let Some(prev) = open.take() {
            flush_run(&mut out, &prev, &run, span);
        }
        out_rows.push(out);
    }
    out_rows
}

/// Everything about a cell that decides how it is painted.
///
/// Named, so that "does this one look like the last one?" is a single
/// comparison rather than seven, and so an attribute added later cannot be
/// half-remembered here
#[derive(Clone, Copy, PartialEq, Eq)]
struct Look {
    fg: vt100::Color,
    bg: vt100::Color,
    inverse: bool,
    bold: bool,
    dim: bool,
    italic: bool,
    underline: bool,
}

/// The whole screen as one string, rows separated by newlines.
///
/// For everything that cannot repair a single row: the phone's relay, an
/// unfocused pane, a test. The window uses `screen_rows` and sends the few
/// rows that moved
pub fn screen_html(screen: &vt100::Screen) -> String {
    screen_rows(screen).join("\n")
}

/// Writes out one run. `span` is the number of cells the run occupies.
///
/// Without writing the width explicitly, a single character from a font
/// whose advance width doesn't match would throw off the rest of that
/// entire line. Both box-drawing characters and CJK text run into this.
fn flush_run(out: &mut String, style: &str, run: &str, span: usize) {
    if run.is_empty() {
        return;
    }
    // Trailing whitespace at the end of a line doesn't need a fixed position (nothing follows it)
    if style.is_empty() && run.trim_end().is_empty() {
        out.push_str(run);
        return;
    }
    // The cell width is measured by the page and supplied via a CSS
    // variable. Using ch (the font's own advance width for "0") instead
    // would give the cursor a different number than the content, and the
    // gap would widen column by column.
    box_of(out, style, run, span, false);
}

/// Writes out a single cell (2 cells wide for full-width characters), containing just that one character.
///
/// A character whose advance width is narrower than the cell is centered.
/// Left-aligning it instead would leave gaps only on the right side of
/// each character, making the row look uneven.
fn flush_cell(out: &mut String, style: &str, ch: &str, span: usize) {
    box_of(out, style, ch, span, true);
}

fn box_of(out: &mut String, style: &str, body: &str, span: usize, center: bool) {
    // The cell width is measured by the page and supplied via a CSS
    // variable. Using ch (the font's own advance width for "0") instead
    // would give the cursor a different number than the content, and the
    // gap would widen column by column.
    out.push_str("<span style=\"display:inline-block;vertical-align:top;width:calc(var(--cw)*");
    out.push_str(&span.to_string());
    out.push_str(");");
    if center {
        out.push_str("text-align:center;overflow:hidden;");
    }
    out.push_str(style);
    out.push_str("\">");
    out.push_str(body);
    out.push_str("</span>");
}

/// Fills in the translated strings and build stamp, producing a page ready to serve.
/// The menu shown on the dashboard (key to press, translation key).
///
/// A pressed key is delivered verbatim as a keystroke while INDEX is in
/// view. Adding a key here that the receiver (INDEX's dispatch) doesn't
/// know about produces "it's shown, but pressing it does nothing".
pub const MENU: [(&str, &str); 9] = [
    ("e", "tui.menu.settings"),
    ("p", "tui.menu.palette"),
    ("f", "tui.menu.vault"),
    ("i", "tui.menu.phone"),
    ("r", "tui.menu.restart"),
    ("w", "tui.menu.workspace"),
    ("t", "tui.menu.notify"),
    ("k", "tui.menu.password"),
    ("?", "tui.menu.help"),
];

/// Menu keys only the window can carry out, and which the remote gate therefore
/// refuses. Settings ("e") is here because a keystroke would only ever reach the
/// window — the phone doesn't send one, it walks to its own `/cfg` page, so the
/// board shows that entry as live from afar all the same.
///
/// One list, two readers: `remote::allowed_from_afar` refuses these, and the
/// board dims the ones it can't perform itself. Adding a window-only item here
/// is all it takes for both sides to agree.
pub const WINDOW_ONLY_MENU: [&str; 3] = [
    // Settings and the browser open as child WebViews inside the window.
    "e", "o",
    // The master password is asked in the TUI, where answering it blocks the app
    // until the person at the window replies.
    "k",
];

/// The sub-input bar's quick actions, as the shell wants them: `text` is the
/// string to insert for a plain action, or null for a Lua one (whose source
/// stays server-side — the shell fires it, it never holds the code).
/// The rebindable actions, as the palette lists them: name to run by, and the
/// translated description to show. Built from the one keys table, so the
/// palette shows exactly what the app can do and nothing it cannot
fn key_actions_json() -> String {
    let rows: Vec<serde_json::Value> = crate::keys::listing()
        .into_iter()
        .map(|(name, desc)| serde_json::json!({ "name": name, "label": crate::i18n::t(desc) }))
        .collect();
    serde_json::to_string(&rows).unwrap_or_else(|_| "[]".into())
}

pub(crate) fn actions_json() -> String {
    let configured = crate::config::actions();
    let list: Vec<serde_json::Value> = if configured.is_empty() {
        // A localized starter set, so the bar isn't empty out of the box (this was
        // the "where are the features?" gap). A user's own `actions` in config
        // replaces these entirely.
        ["continue", "explain", "review", "fix"]
            .iter()
            .map(|k| {
                serde_json::json!({
                    "label": crate::i18n::t(&format!("actions.default.{k}.label")),
                    "text": crate::i18n::t(&format!("actions.default.{k}.body")),
                    "lua": false,
                })
            })
            .collect()
    } else {
        configured
            .into_iter()
            .map(|a| {
                serde_json::json!({
                    "label": a.label,
                    "text": if a.lua { serde_json::Value::Null } else { serde_json::Value::String(a.body) },
                    "lua": a.lua,
                })
            })
            .collect()
    };
    serde_json::to_string(&list).unwrap_or_else(|_| "[]".into())
}

pub fn page() -> String {
    page_for(false)
}

/// The shell with the phone's pairing mode baked in (sticky: keep the
/// token in the URL and persistent storage — see RemoteSpec::sticky_token).
/// The window never pairs, so page() serves it the cautious default
pub fn page_for(sticky: bool) -> String {
    // Read here rather than threaded in: the page is built in several places
    // (window, phone, tests) and every one of them wants the same look
    let look = crate::config::load().map(|c| c.appearance).unwrap_or_default();
    let scheme = look.scheme();
    let dict = crate::i18n::dict_json();
    let keys: Vec<&str> = MENU.iter().map(|(k, _)| *k).collect();
    let words: std::collections::BTreeMap<&str, &str> = MENU.iter().copied().collect();
    // The message toast is the app's, not this screen's — every surface that
    // says anything to the user says it the same way (src/toast.rs)
    crate::toast::render(PAGE.to_string()).replace(
        "{{MENU_KEYS}}",
        &serde_json::to_string(&keys).unwrap_or_else(|_| "[]".into()),
    )
    .replace(
        "{{MENU_WORDS}}",
        &serde_json::to_string(&words).unwrap_or_else(|_| "{}".into()),
    )
    .replace(
        "{{MENU_WINDOW_ONLY}}",
        &serde_json::to_string(&WINDOW_ONLY_MENU).unwrap_or_else(|_| "[]".into()),
    )
    .replace("{{__lang__}}", &crate::i18n::lang())
    .replace("{{FONT}}", &look.font_css())
    .replace("{{FONT_SIZE}}", &look.size_px().to_string())
    .replace("{{TAB_W}}", &crate::config::tab_bar_px().to_string())
    .replace("{{TAB_W_MIN}}", &crate::config::TAB_BAR_MIN_PX.to_string())
    .replace("{{TAB_W_MAX}}", &crate::config::TAB_BAR_MAX_PX.to_string())
    .replace("{{TAB_W_DEF}}", &crate::config::TAB_BAR_DEFAULT_PX.to_string())
    .replace("{{THEME}}", &scheme.css_vars())
    .replace(
        "{{SCHEME}}",
        if crate::theme::is_light(&scheme) { "light" } else { "dark" },
    )
    .replace("{{DICT}}", &dict)
    .replace("{{KEY_ACTIONS}}", &key_actions_json())
        .replace(
            "{{CAST_KEYS}}",
            &serde_json::to_string(&crate::config::cast_keys()).unwrap_or_else(|_| "[]".into()),
        )
        .replace("{{ACTIONS}}", &actions_json())
        .replace("{{STICKY}}", if sticky { "true" } else { "false" })
        .replace(
        "{{BUILD}}",
        &serde_json::to_string(&format!(
            "build {}  ({})",
            env!("BUILD_TIME"),
            env!("BUILD_REV")
        ))
        .unwrap_or_else(|_| "\"\"".into()),
    )
}

#[cfg(test)]
mod tests {
    use super::{PAGE, screen_html, screen_rows};

    /// Every message this screen shows goes through the one shared toast.
    ///
    /// It used to have two of its own: a message line fed by the app's state
    /// that only went away on the next keystroke, and a separate attach toast.
    /// Neither could be dismissed by hand. If a second implementation ever
    /// creeps back in, one kind of message starts behaving unlike the rest.
    #[test]
    fn every_message_is_the_shared_toast() {
        let page = super::page_for(false);
        for once in ["let toastTimer", "function toast(", "function hideToast("] {
            assert_eq!(page.matches(once).count(), 1, "{once} が重複または欠落している");
        }
        assert!(
            !page.contains("attachtoast") && !page.contains("attachToast"),
            "この画面だけの古いトーストが残っている"
        );
        assert!(
            !page.contains(r#"getElementById("flash")"#),
            "消えないメッセージ行が残っている"
        );
        // The app's message is state, so it must be shown by comparing it —
        // otherwise every repaint would put a dismissed message straight back up
        assert!(
            page.contains("if (S.flash !== lastFlash)"),
            "アプリからのメッセージが値の変化で出ていない"
        );
    }


    /// Every translation key the page builds at run time must exist too.
    ///
    /// A missing key just becomes an empty string — no crash, no warning —
    /// so the only way to notice is "I pressed it and nothing appeared".
    /// This actually happened: help was reading a nonexistent key,
    /// tui.help.body, and showed an empty box with just a title. It looked
    /// like pressing it simply did nothing.
    ///
    /// Keys written out in full are checked for every page at once, in
    /// `i18n::tests::every_word_a_page_asks_for_exists`. What is left here is
    /// the half that test cannot do: a key half of which is decided while the
    /// page runs, so only the page's own list can say what to look for.
    #[test]
    fn every_word_the_page_builds_is_in_the_dictionary() {
        let en: serde_json::Value =
            serde_json::from_str(include_str!("../lang/en.json")).unwrap();
        let p = super::page();
        let mut checked = 0;
        // The dynamically-built form (T["tui.help." + k]). The names are read
        // out of the page's own list rather than copied into this test: a
        // second list would be a second thing to keep in step, and the one
        // that fell behind would be the one nobody looked at
        let head = "T[\"tui.help.\" + k]";
        if let Some(at) = p.find(head) {
            let opened = p[..at]
                .rfind("for (const k of [")
                .expect("動的に読む一覧が見つからない");
            let list = &p[opened..at];
            let list = &list[list.find('[').unwrap() + 1..list.find(']').expect("閉じていない")];
            for k in list.split(',') {
                let k = k.trim().trim_matches('"');
                if k.is_empty() {
                    continue;
                }
                let key = format!("tui.help.{k}");
                assert!(en.get(&key).is_some(), "lang/en.json に無いキー: {key}");
                checked += 1;
            }
        }
        // Not a count: the list is as long as it happens to be. What matters is
        // that the form is still there to be read, and that reading it found
        // something -- a silent zero is how a check stops checking
        assert!(p.contains(head), "動的に読む形が消えた: {head}");
        assert!(checked > 0, "動的な訳語を読めていない");
    }

    /// The board's "edit settings" entry must never be forwarded as a keystroke.
    ///
    /// A keystroke for it only ever lands in the window, so from a phone the
    /// entry looked alive and did nothing at all: the remote gate refused it and
    /// the board had no other way to open settings. The board now performs that
    /// entry itself (the window opens its child WebView, the phone walks to the
    /// reverse-proxied /cfg page), and this pins the three pieces that has to
    /// stand on: the entry exists, its keystroke is still window-only (the gate
    /// reads the same list), and the board claims it by name.
    #[test]
    fn the_board_opens_settings_itself_rather_than_sending_a_key() {
        let (key, word) = super::MENU
            .iter()
            .find(|(_, w)| *w == "tui.menu.settings")
            .copied()
            .expect("盤面に設定の項目が無い");
        assert!(
            super::WINDOW_ONLY_MENU.contains(&key),
            "設定の打鍵は窓にしか届かない。遠隔の門は {key} を通してはいけない"
        );
        // The board performs settings itself rather than forwarding a keystroke.
        // Checked as one entry in MENU_OWN, not the whole object, so adding
        // another self-performed entry (the Vault) does not trip this
        assert!(
            super::page().contains(&format!("\"{word}\": () => openSettings()")),
            "盤面が設定の項目を自前で担っていない"
        );
    }

    /// An entry the window alone can carry out, and which the board can't perform
    /// itself either, must be visibly unavailable from afar. Anything else is a
    /// button that silently does nothing, which reads as a broken app.
    ///
    /// The master password is the standing case: it's asked in the TUI, where
    /// answering it blocks the app until the person at the window replies.
    #[test]
    fn an_entry_the_phone_cannot_reach_is_marked_as_such() {
        let page = super::page();
        let unreachable: Vec<&str> = super::WINDOW_ONLY_MENU
            .iter()
            .filter(|key| super::MENU.iter().any(|(k, _)| k == *key))
            .filter(|key| {
                let (_, word) = super::MENU.iter().find(|(k, _)| k == *key).unwrap();
                !page.contains(&format!("\"{word}\": () => "))
            })
            .copied()
            .collect();
        assert!(
            !unreachable.is_empty(),
            "窓専用の項目が盤面から消えたなら、この検査ごと畳んでよい"
        );
        for key in unreachable {
            assert!(
                page.contains("MENU_WINDOW_ONLY.includes(k)"),
                "{key}: 遠くからは押せないのに、盤面がそれを見せていない"
            );
        }
        assert!(
            page.contains("windowonly") && page.contains("tui.menu.window_only"),
            "印(見た目と但し書き)が page から失われている"
        );
    }

    /// While a press is in progress, the dashboard must not be rebuilt.
    ///
    /// A click only registers when "pointerdown and pointerup happen on the
    /// same element". Since the dashboard used to be fully rebuilt every
    /// time state arrived, a rebuild mid-press meant the pressed element no
    /// longer existed, and the press never reached anywhere. Since the
    /// activity graph keeps updating constantly, this wasn't some rare edge
    /// case — it was the default behavior, and INDEX's menu couldn't be
    /// clicked with a mouse at all.
    ///
    /// This is the kind of bug you can only notice by watching it happen, so it's pinned down here.
    #[test]
    fn a_press_is_not_interrupted_by_a_redraw() {
        let p = super::page();
        // The redraw entry point must hold back updates while a press is in progress
        let at = p.find("window.__state = function").expect("状態の入口が無い");
        let head = &p[at..at + 200];
        assert!(
            head.contains("holding") && head.contains("queued"),
            "状態が届いたら、押している最中でも作り直してしまう"
        );
        // On release, any held-back redraw must be flushed (so the screen doesn't freeze after a press)
        assert!(
            p.contains("addEventListener(\"pointerup\", release"),
            "離したときに、預かった描き直しを流していない"
        );
        // The escape hatch for when the pointer is released outside the window. Without it, a stuck "held" state freezes the screen
        assert!(
            p.contains("addEventListener(\"pointercancel\", release")
                && p.contains("addEventListener(\"blur\", release"),
            "押しっぱなしのまま画面が止まる道が残っている"
        );
    }

    /// The distributed page must have no unfilled placeholders left.
    ///
    /// A leftover placeholder turns the whole page into a SyntaxError, and
    /// the screen shows nothing with no visible cause.
    #[test]
    fn the_page_has_nothing_left_to_fill_in() {
        // Don't initialize the language here — doing so would change the
        // language for other tests running concurrently (a dashboard test
        // once failed this way looking for CHAIN)
        let p = super::page();
        assert!(!p.contains("{{"), "差し込み先が残っている");
        assert!(p.contains("const T = {"), "訳語が入っていない");
        // Every colour the page draws with, including the terminal's sixteen
        assert!(p.contains("--bg:") && p.contains("--c15:"), "配色が入っていない");
        assert!(
            p.contains("color-scheme:dark") || p.contains("color-scheme:light"),
            "ブラウザ側が描く部分の明暗が指定されていない"
        );
        assert!(p.contains("const BUILD = \""), "ビルド刻印が入っていない");
        // A stale page (a phone keeping the board open across app updates)
        // must reload itself, and the 🎯 picker must exclude plain terminals
        assert!(p.contains("S.build !== BUILD"), "古いページの自動リロードが無い");
        assert!(
            p.contains("t.kind === \"browser\" || t.ai"),
            "🎯候補がAI/ブラウザに絞られていない"
        );
        // The 🎯 panel itself exists only on AI-operator tabs; a plain
        // terminal gets 🤖 instead, and its pen is a color emoji (the text
        // glyph ✎ has no glyph in some fonts — pressable but invisible)
        assert!(
            p.contains("if (t && t.ai) return base.concat(\"target\")"),
            "🎯パネルがAIタブ限定になっていない"
        );
        assert!(p.contains("✏️"), "ペンがカラー絵文字になっていない");
        // A horizontal accent rule says "the focus is here" and nothing else.
        // The composer's own seam once said it too, inside the pane the
        // focused caption had just underlined, and the eye read a pane
        // boundary where there was none
        assert!(
            !p.contains("background:var(--panel); border-top:1px solid var(--brand); }"),
            "入力欄の継ぎ目がフォーカス線と同じ青を使っている"
        );
        assert!(
            p.contains(".pane.focused .phead { color:var(--text); background:var(--raise);"),
            "フォーカス中のペインの見出しが見分けられない"
        );
        // Two panes showing nothing must still look like two panes. The
        // divider carries the only line there is -- the panes have no border
        assert!(
            p.contains(".pdiv::after { content:\"\"; position:absolute; background:var(--line); }"),
            "ペイン同士の境目に線が引かれていない"
        );
        assert!(!p.contains("\"✎\""), "見えない文字グリフのペンが残っている");
    }

    /// The composer belongs to the pane you are typing at, not to the window.
    ///
    /// It was pinned to the window's floor because the phone's keyboard
    /// handler assigned its height straight to `bottom`, overwriting the
    /// pane offset the stylesheet had put there. Summoned from the top pane,
    /// the bar opened at the bottom of the screen -- under a different pane
    /// entirely, or hidden beneath a browser placed in one. The lift is a
    /// separate term now, and nothing may write the whole of `bottom` again.
    #[test]
    fn the_composer_opens_at_the_foot_of_the_pane_that_summoned_it() {
        let p = super::page();
        assert!(
            p.contains("bottom:calc(var(--fb) + var(--kbd, 0px))"),
            "サブ入力欄の位置が、フォーカス中のペイン基準になっていない"
        );
        assert!(
            !p.contains("castDock.style.bottom"),
            "キーボード分の持ち上げがペインの位置を上書きしている"
        );
        // The pen that summons it is anchored to the same pane
        assert!(
            p.contains("bottom:calc(var(--fb) + 16px)"),
            "ペンがフォーカス中のペインに付いていない"
        );
        // A browser placed in a pane is a native window drawn over this page,
        // so the moment it forgets its pane it covers the pen and the pane
        // below. Its bottom is composed the same way, and the room held back
        // for the composer is a term added to it, never the whole of it
        assert!(
            p.contains("bottom:calc(var(--fb) + var(--dock, 0px))"),
            "ブラウザの位置が、フォーカス中のペイン基準になっていない"
        );
        assert!(
            !p.contains("page.style.bottom"),
            "ドックのぶんの余白がペインの位置を上書きしている"
        );
    }


    /// A message belongs to the pane in front, and is drawn where it can be
    /// read whole.
    ///
    /// Centred on the window, a toast was cut in two by a page placed in the
    /// other half: a page is a window of its own, and no z-index puts anything
    /// above it. So the window seats its toast over the FOCUSED pane — and when
    /// that pane holds such a page, the page draws the message itself (see
    /// caps::browser_toast and the placed-page script).
    #[test]
    fn a_message_is_seated_over_the_pane_it_is_about() {
        let p = super::page();
        assert!(
            p.contains("--toast-x:calc(var(--fx) + (100% - var(--fx) - var(--fr)) / 2)"),
            "トーストが窓の中央のまま（ペインに座っていない）"
        );
        assert!(
            p.contains("--toast-bottom:calc(var(--fb) + 52px)"),
            "トーストがペインの底ではなく窓の底に付いている"
        );
        assert!(
            p.contains("--toast-max:min(calc(100% - var(--fx) - var(--fr) - 24px), 560px)"),
            "狭いペインでトーストがはみ出す"
        );
    }

    /// A hidden pane is not a pane, and nothing may be placed by its rectangle.
    ///
    /// INDEX and the settings form cover the panes, so `#panes` is hidden --
    /// and a hidden element reports a rectangle of zeros rather than no
    /// rectangle at all. Measured as if it were a pane, those zeros describe
    /// one sitting off the top-left corner of the window, and everything seated
    /// on it went there too: on INDEX the toast was drawn at left:-290px,
    /// bottom:998px in a 900px-tall window. Every board menu item that answers
    /// with a message ("no notification target", "no exited tabs", "phone
    /// access is off") therefore looked like a button that does nothing at all
    /// -- on a brand-new install, which is the first thing anyone tries.
    #[test]
    fn a_covered_pane_is_the_window() {
        let p = super::page();
        assert!(
            p.contains("const laid = f && f.getClientRects().length > 0;")
                && p.contains("const b = laid ? f.getBoundingClientRect() : m;"),
            "隠れたペインの矩形（全部ゼロ）をそのまま使っている"
        );
        // One answer to "is a screen covering the panes", and the things that
        // belong to a pane all ask it
        assert_eq!(
            p.matches("const covering = () =>").count(),
            1,
            "「ペインが覆われているか」の答えが1箇所ではない"
        );
        assert!(
            p.contains("if (covering() || (REMOTE && (screen.hidden || web || !onTermPty()))) closeBar();"),
            "打ち込む先が無い画面でサブ入力欄が開いたままになる"
        );
        assert!(
            p.contains("const here = !covering()"),
            "打ち込む先が無い画面にペンが出る"
        );
    }

    /// A covered pane has not changed size, and must not be told that it has.
    ///
    /// The same zeros that misplaced the toast were also being measured as a
    /// terminal, where they come out as the smallest `fit` will name: 20x5. So
    /// opening INDEX -- or the settings form -- told every running AI that its
    /// window had shrunk to a fifth of a screen. Qwen, Claude Code, anything
    /// built on Ink reflowed its whole interface to suit, and was told to grow
    /// back the moment the cover lifted. What survived that round trip was a
    /// broken frame: blank rows, and the box it had drawn cut off mid-line. It
    /// repaired only where the next keystroke made the program draw again,
    /// which is why typing appeared to fix it a piece at a time.
    ///
    /// Measured on a running window, the resize sent on opening INDEX went
    /// 118x47 -> 20x5 -> 118x47. It now stays 118x47 throughout.
    #[test]
    fn a_covered_pane_keeps_the_size_it_had() {
        let p = super::page();
        assert!(
            p.contains("&& boxes.every((el) => el.querySelector(\".pbody\").getClientRects().length > 0);"),
            "覆われたペインの矩形（全部ゼロ）から行数・桁数を出している"
        );
        assert!(
            p.contains("if (laid) lastPanes = boxes.map((el) => {"),
            "ペインが覆われている間の測定値を採用してしまう"
        );
        assert!(
            p.contains("const panes = lastPanes || [];") && p.contains("const f = lastFit || fit("),
            "覆われている間、最後にちゃんと測れた値を使っていない"
        );
    }

    /// There is one sub-input bar, and every pane types into it.
    ///
    /// A model pane used to carry a composer of its own -- its own textarea,
    /// its own Send, its own Enter handling, its own growth cap -- pinned to
    /// the bottom of the pane. Two composers meant two answers to every
    /// question asked of one (what the Enter key does, what a paste does, where
    /// an attachment goes, whether the key row is reachable), and they had
    /// already drifted: the quick actions and the phone's key row existed on
    /// one of them only. What differs between panes is not the field, it is
    /// where a Send goes -- and that is one line in sendBar().
    #[test]
    fn one_composer_serves_every_pane() {
        let p = super::page();
        assert!(
            !p.contains("modelchat"),
            "モデルのペインが自前の入力欄を持っている（サブ入力欄が2つある）"
        );
        assert_eq!(
            p.matches("id:\"castinput\"").count(),
            1,
            "サブ入力欄を組み立てる場所が1つではない"
        );
        // One place decides where a finished line goes, and both doors that
        // finish a line go through it
        assert!(p.contains("function sendLine(text, tab) {"), "行の渡し方を決める一箇所が無い");
        assert!(p.contains("    sendLine(t);"), "サブ入力欄の Send が自前で送っている");
        assert!(
            p.contains("sendLine(topic, S.discuss_start);"),
            "討論の議題欄が自前で送っている（口火役がモデルだと届かない）"
        );
        // ...and the line always says who it is for. Delivered to "whoever is
        // in front", the topic box's own view switch could arrive after it
        assert_eq!(
            p.matches("kind:\"say\"").count(),
            1,
            "行を渡す口が1つではない"
        );
        assert!(
            p.contains(r#"send({kind:"say", tab: (tab == null ? S.active : tab), text}); return;"#),
            "渡す行に宛名が付いていない"
        );
        // Actions only there (plus the key row on a phone) -- no 🎯, no 🤖, no 📼
        assert!(
            p.contains("if (t && t.model) return base;"),
            "モデルのペインにアクション以外のパネルが出る"
        );
        assert!(
            p.contains(r#"const base = (typeof REMOTE !== "undefined" && REMOTE) ? ["keys", "actions"] : ["actions"];"#),
            "スマホの特殊キーが基本パネルから外れている"
        );
    }

    /// The ✏️ pen is decided by where we are now, not by where we were when
    /// the composer was closed.
    ///
    /// Closing the bar over a placed page hides the window's own pen — that
    /// page draws one for itself, and ours would be underneath it — and the
    /// answer used to be frozen at that moment. Walk to an AI tab afterwards
    /// and there was no pen and no way back into the composer, because nothing
    /// ever asked the question again. One function owns the answer and every
    /// state push re-asks it.
    #[test]
    fn the_pen_is_decided_by_where_we_are_now() {
        let p = super::page();
        assert!(p.contains("function syncPen()"), "ペンの可否を決める一箇所が無い");
        assert_eq!(
            p.matches("fab.style.display =").count(),
            1,
            "ペンの表示を書く場所が2つ以上ある（片方が場所を決め打ちして固まる）"
        );
        assert!(
            p.contains(r#"(typeof REMOTE !== "undefined" && REMOTE) ? onTermPty() : !onBrowserTab()"#),
            "どこに居るかの判定が入っていない"
        );
        // Both ways in and out of the composer, plus every state push, go
        // through it — the last one is what makes walking to another tab work
        assert!(
            p.contains("castDock.style.display = \"flex\"; syncPen();")
                && p.contains("syncPen();\n  drawTabs();"),
            "開閉と状態更新のどこかが自分で決めている"
        );
    }

    /// The relayed picture is the size of the pane it sits in, never the size
    /// of the frame arriving.
    ///
    /// A canvas is a replaced element. Leave its width `auto` between a `left`
    /// and a `right` and the browser keeps the frame's own pixel size and drops
    /// the `right` as over-constrained. On a phone that frame is the PC's page:
    /// twice the width of the screen, with half of it hanging off the right
    /// edge and no way to reach it. Worse, the phone reports its screen shape
    /// from this very box — so it reported the frame's own shape straight back,
    /// the PC never re-shaped the page to suit the phone, and the black band
    /// under the picture could never close.
    #[test]
    fn the_relayed_picture_is_the_size_of_the_pane_it_sits_in() {
        let p = super::page();
        assert!(
            p.contains("width:calc(100% - var(--fx) - var(--fr));"),
            "中継画面の幅がペインの幅になっていない"
        );
        assert!(
            p.contains("height:calc(100% - var(--fy) - var(--navh) - var(--fb));"),
            "中継画面の高さがペインの高さになっていない"
        );
        assert!(
            !p.contains("width:auto; height:auto;"),
            "置換要素の寸法を auto に戻すと、届いたフレームの原寸で描かれる"
        );
    }

    /// The tab bar is a width, and putting it away is that width being zero.
    ///
    /// Two pieces of state -- a width and a "hidden" flag -- would be two
    /// answers to one question, and the day they disagreed the bar would be
    /// nowhere with a width, or somewhere with none. One number also means the
    /// drag and the keyboard write the same thing, and the settings file holds
    /// exactly what the window is showing.
    #[test]
    fn the_tab_bar_is_one_number_wide() {
        let p = super::page();
        assert!(p.contains("#tabs { grid-row:1/3; width:var(--tabw);"), "タブバーの幅が固定のまま");
        // The grip never leaves the screen, or a bar put away could not be
        // pulled back out
        assert!(
            p.contains("left:max(0px, calc(var(--tabw) - 4px))"),
            "しまったタブバーを掴み直せる位置に取っ手が無い"
        );
        assert!(p.contains("window.__toggleTabBar"), "キーからしまう入口が無い");
        // A drag switches off every pointer target but the handles. Leave the
        // grip out of that list and its own double-click stops arriving
        assert!(
            p.contains("body.dragdiv .pdiv, body.dragdiv #tabgrip { pointer-events:auto; }"),
            "ドラッグ中に取っ手自身がポインタを失う"
        );
        // The bounds are the app's, handed in rather than written twice
        assert!(
            !p.contains("{{TAB_W"),
            "幅の値がページに差し込まれていない"
        );
        assert!(
            p.contains(&format!("const TABW_MIN = {}", crate::config::TAB_BAR_MIN_PX)),
            "ページとアプリで下限が食い違っている"
        );
    }

    /// Elements marked hidden must actually be hidden.
    ///
    /// HTML's hidden attribute defaults to display:none, but declaring
    /// display yourself overrides that default and un-hides it. This once
    /// left an overlay stuck visible, with the screen dark and nothing clickable.
    ///
    /// This is a class of bug you can only notice by looking at the
    /// rendered result, so it's pinned down here.
    #[test]
    fn things_marked_hidden_are_actually_hidden() {
        // Collect every id in the markup that has hidden set
        let mut ids: Vec<String> = Vec::new();
        for line in PAGE.lines() {
            let t = line.trim();
            if !t.contains("hidden") || !t.contains("id=\"") {
                continue;
            }
            if let Some(rest) = t.split("id=\"").nth(1) {
                if let Some(id) = rest.split('"').next() {
                    ids.push(id.to_string());
                }
            }
        }
        assert!(!ids.is_empty(), "hidden を使っている要素が見つからない");

        for id in ids {
            // Whether that id has its own display rule
            let sets_display = PAGE.lines().any(|l| {
                let t = l.trim_start();
                t.starts_with(&format!("#{id} "))
                    && !t.contains("[hidden]")
                    && t.contains("display:")
            });
            if sets_display {
                assert!(
                    PAGE.contains(&format!("#{id}[hidden]")),
                    "#{id} は display を書いているのに、hidden で消す指定が無い"
                );
            }
        }
    }

    /// No name is declared twice.
    ///
    /// This once broke the settings page entirely: a duplicate declaration
    /// becomes a SyntaxError, and the whole script fails to run. All that's
    /// left on screen is a heading, with no visible cause.
    #[test]
    fn nothing_is_declared_twice() {
        let mut seen: Vec<&str> = Vec::new();
        for line in PAGE.lines() {
            let t = line.trim_start();
            for kw in ["const ", "let ", "function "] {
                let Some(rest) = t.strip_prefix(kw) else {
                    continue;
                };
                // An indented line is inside a function body, so a duplicate there is fine
                if line.starts_with(' ') {
                    continue;
                }
                let name: &str = rest
                    .split(|c: char| !(c.is_alphanumeric() || c == '_'))
                    .next()
                    .unwrap_or("");
                if name.is_empty() {
                    continue;
                }
                assert!(!seen.contains(&name), "{name} が2回宣言されている");
                seen.push(name);
            }
        }
        assert!(seen.contains(&"send"), "走査が効いていない");
    }

    /// Every field of the state must be used somewhere on the page.
    ///
    /// A field that's sent but never read by anything becomes the first
    /// suspect when hunting down "something that should appear doesn't"
    #[test]
    fn every_piece_of_state_is_used() {
        for field in [
            "workspace", "active", "auto_enabled", "remote_on", "tabs", "flash",
            "holder", "depth", "max", "awaiting_human", "locked", "profile",
            "activity", "state", "name", "index",
        ] {
            assert!(PAGE.contains(field), "状態の {field} を誰も見ていない");
        }
    }

    /// Only the window losing focus may end a press.
    ///
    /// The guard was released on a CAPTURING blur listener, which hears every
    /// element's blur, not the window's. Clicking a tab blurs whatever had focus,
    /// so the guard was disarmed in the middle of the very press it protects: the
    /// next state push rebuilt the tab bar, the element went out from under the
    /// click, and the press was never delivered. Measured at 3 in 10 on a real
    /// window, worse right after switching to a browser tab, where the page load,
    /// the nav bar and the screencast all push at once. blur does not bubble, so
    /// a plain listener hears the window alone.
    #[test]
    fn a_press_ends_only_when_the_window_does() {
        assert!(
            PAGE.contains(r#"addEventListener("blur", release);"#),
            "ウィンドウ以外の blur でも押下ガードを解除している"
        );
        assert!(
            !PAGE.contains(r#"addEventListener("blur", release, true);"#),
            "capture 付きの blur に戻っている (要素の blur でガードが外れる)"
        );
        // The presses themselves still have to be seen before anything else
        for armed in [r#"addEventListener("pointerdown""#, r#"addEventListener("pointerup", release, true)"#] {
            assert!(PAGE.contains(armed), "押下の検知が消えている: {armed}");
        }
    }

    /// A press must survive the redraw that ends it.
    ///
    /// State arriving mid-press is held back, because rebuilding the tab bar
    /// between pointerdown and pointerup destroys the element and the press goes
    /// nowhere. But the click is delivered AFTER pointerup, so releasing the hold
    /// and redrawing in the same handler recreated the very bug the hold exists
    /// to prevent — a tab click was swallowed whenever a push happened to land
    /// during the press, which is most of the time just after switching to a
    /// browser tab (its load, nav bar and screencast all push at once).
    #[test]
    fn a_press_outlives_the_redraw_that_ends_it() {
        assert!(
            PAGE.contains("setTimeout(() => window.__state(j), 0);"),
            "押している間に溜めた再描画を、その場で流している (クリックが消える)"
        );
        assert!(
            !PAGE.contains("queued = null; window.__state(j); }"),
            "pointerup と同じ処理の中で再描画する古い配線に戻っている"
        );
    }

    /// Where the restart button appears is the app's call, not the screen's.
    ///
    /// It first showed only on terminal tabs, on the reasoning that a page has its
    /// own reload — but a reload cannot take a page back to where it started, and
    /// some panes have no reload wired at all. The app now says which panes can be
    /// put back (`restartable`), and the screen must read that rather than working
    /// it out again and disagreeing.
    #[test]
    fn the_restart_button_follows_what_the_app_says() {
        assert!(
            PAGE.contains("if (!REMOTE || !(S && S.restartable)) { restartArmed = 0; return null; }"),
            "再起動ボタンが本体の判断を読んでいない"
        );
        assert!(
            PAGE.contains("b.hidden = !(t && t.restartable);"),
            "ペインの再起動ボタンが本体の判断を読んでいない"
        );
        assert!(
            !PAGE.contains("if (!onTerminal()) { restartArmed = 0; return null; }"),
            "画面側が独自に判断する古い配線に戻っている"
        );
    }

    /// Restarting belongs to a pane, not to the status bar — at the window.
    ///
    /// A status bar has one button and a divided screen has several panes, so the
    /// bar's ↻ could only ever reach whichever pane had focus: the other half of a
    /// split could not be restarted without first going and standing in it. The
    /// pair now lives in each pane's caption, where it names the pane it is drawn
    /// on. The bar keeps it on the phone alone, which has no panes to be ambiguous
    /// about and is exactly who needs it when an SSH tab drops.
    #[test]
    fn a_pane_is_restarted_from_its_own_caption() {
        // Both meanings offered, because one control that picks for you is how
        // a restart eats a day's work
        assert!(
            PAGE.contains(r#"send({kind:"restartpane", id:p.id, keep});"#),
            "ペインの見出しから再起動を送っていない"
        );
        assert!(
            PAGE.contains(r#"for (const [cls, keep] of [[".rk", true], [".rf", false]])"#),
            "引き継ぐ/引き継がないの二つが揃っていない"
        );
        // A live pane asks twice, and only one arming is live at a time
        assert!(
            PAGE.contains(r#"if (armedPane === cls + p.id || (t && t.state === "EXIT"))"#),
            "動いているペインを一押しで落とせてしまう"
        );
    }

    /// The tab bar's + has to work on a phone too.
    ///
    /// It used to send the addtab intent on every surface, but that intent turns
    /// into the keystroke that opens the settings as a child WebView — something
    /// only the window has. `allowed_from_afar` refused it, so from a phone the +
    /// did nothing at all, silently. The phone walks to the settings page instead.
    #[test]
    fn the_tab_bar_plus_reaches_the_settings_from_a_phone() {
        assert!(
            PAGE.contains("walkToSettings({addtab: (S && S.ws_index) || 0})"),
            "スマホの + が設定ページへ歩いて行かない"
        );
        // The window still takes the keystroke path (the WebView is its to open)
        assert!(PAGE.contains(r#"else send({kind:"addtab"});"#), "窓側の道が消えている");
        assert!(
            !PAGE.contains(r#"onclick:() => send({kind:"addtab"})"#),
            "+ が全surface共通で intent を送る古い配線に戻っている"
        );
    }

    /// The ✕ on the sub-input bar has to stick.
    ///
    /// On a phone, tapping the terminal summons the composer — and that tap used
    /// to clear the ✕ as well, so the bar came back the instant the screen was
    /// touched and the ✕ meant nothing (there was no way to reach the screen
    /// without it). Every path that opens the bar on its own asks castClosed()
    /// first; only the ✎ pen clears the choice, and only when pressed -- the
    /// one the window draws and the one a placed page draws for itself are the
    /// same pen and go through the same door, which is why there is still
    /// exactly one place that clears it.
    #[test]
    fn dismissing_the_composer_survives_a_tap() {
        assert!(
            PAGE.contains("if (REMOTE && onTermPty()) { if (!castClosed()) openTermBar(); return; }"),
            "画面タップが✕を無視して入力欄を開き直す"
        );
        // One reader, so the meaning of the ✕ can't drift between callers
        assert_eq!(
            PAGE.matches(r#"localStorage.getItem("shikishaCastClosed2")"#).count(),
            1,
            "✕の記憶を読む場所が複数ある (食い違いのもと)"
        );
        // Clearing it stays a deliberate act: the pen's toggle, nothing else
        assert_eq!(
            PAGE.matches("rememberCastClosed(false)").count(),
            1,
            "✕の解除が増えている (勝手に開き直る道が復活している)"
        );
    }

    /// A browser must never machine-translate this page.
    ///
    /// Chrome on a phone read the terminal's English output, decided the whole
    /// page was English, and translated it — which swapped the text inside runs
    /// that `box_of` had given a fixed cell width, so the rows landed on top of
    /// one another. The served page has to say no, and say which language it is
    /// so the mis-detection doesn't start.
    #[test]
    fn the_page_is_never_machine_translated() {
        let html = super::page();
        assert!(html.contains("translate=\"no\""), "ページ全体の翻訳拒否が無い");
        assert!(
            html.contains(r#"<meta name="google" content="notranslate">"#),
            "翻訳の申し出を止める meta が無い"
        );
        assert!(
            html.contains(r#"<pre id="screen" class="notranslate""#),
            "端末の中身が個別に守られていない"
        );
        // A real language code, not the raw placeholder
        let lang = crate::i18n::lang();
        assert!(!lang.is_empty() && !lang.contains('{'), "lang が空/未置換: {lang}");
        assert!(
            html.contains(&format!("<html lang=\"{lang}\"")),
            "lang 属性が入っていない (翻訳の誤検出はここから始まる)"
        );
        assert!(!html.contains("{{__lang__}}"), "lang のプレースホルダが残っている");
    }

    /// The terminal is handed over as rows, and the page keeps them as
    /// separate elements.
    ///
    /// This is what lets a changed line be repaired on its own. Go back to one
    /// blob of HTML and every frame an AI draws while thinking rebuilds every
    /// element on screen -- which the person typing into the composer pays for,
    /// because the browser must finish that layout before it can answer how
    /// tall their text box now is.
    #[test]
    fn the_screen_is_made_of_rows_that_can_be_repaired_one_at_a_time() {
        let mut p: vt100::Parser = vt100::Parser::new(4, 20, 0);
        p.process(b"one\r\ntwo\r\nthree");
        let rows = screen_rows(p.screen());
        assert_eq!(rows.len(), 4, "行の数が画面の高さと違う: {rows:?}");
        assert!(
            rows.iter().all(|r| !r.contains('\n')),
            "1行の中に改行が混ざっている: {rows:?}"
        );
        // The one-string form is still the same picture (the phone, an
        // unfocused pane and the tests all read it)
        assert_eq!(screen_html(p.screen()), rows.join("\n"));
        assert!(PAGE.contains("window.__rows = function"), "行だけを直す口が無い");
        assert!(
            PAGE.contains(r#"'<div class="r">'"#),
            "行が別々の要素になっていない"
        );
        assert!(
            PAGE.contains("#screen .r { min-height:1.25em; }"),
            "空の行が高さを失う"
        );
    }

    /// Text selection is limited to the terminal's contents.
    ///
    /// If everything were a single grid of cells, selecting the output
    /// would drag the tab bar and box-drawing lines along with it. Keeping
    /// them separate is exactly what lets only the output be selected.
    #[test]
    fn only_the_terminal_contents_are_selectable() {
        assert!(
            PAGE.contains("#screen { user-select:text; }"),
            "ターミナルの中身が選べる指定が無い"
        );
        assert!(
            PAGE.contains(".tab") && PAGE.contains("user-select:none"),
            "タブバーが選択に混ざる"
        );
    }

    /// The top bar is drawn by the shell, not injected into the page.
    ///
    /// Injecting it into the page would fight with the site's own CSS,
    /// disappear on every navigation, and cover the site's own fixed
    /// header from above. Pushing the page down a notch and drawing in the
    /// space that opens up avoids all of that.
    #[test]
    fn the_bar_is_drawn_by_the_app_not_injected_into_the_page() {
        assert!(PAGE.contains("id=\"nav\""), "バーの置き場所が無い");
        assert!(PAGE.contains("id=\"page\""), "ページを置く場所が無い");
        // Where the page sits is pushed down by exactly the bar's height.
        // Reserved out of the focused pane's rectangle rather than written onto
        // each layer: with panes, "the top" is no longer the top of the window
        assert!(
            PAGE.contains("setProperty(\"--navh\", n.hidden ? \"0px\" : \"36px\")"),
            "バーを出してもページが下がらない"
        );
        // The page and the relay canvas are pushed down by the same amount
        // (otherwise the browser's top edge would hide behind the bar)
        assert_eq!(
            PAGE.matches("top:calc(var(--fy) + var(--navh))").count(),
            2,
            "バーを出しても #page と中継キャンバスの両方は下がらない"
        );
        // Rows/columns come from #main; the browser view's placement comes
        // from #page. Deriving both from one rectangle would shrink the
        // terminal just because the bar appeared
        assert!(
            PAGE.contains("document.getElementById(\"page\").getBoundingClientRect()"),
            "置き場所を #page から取っていない"
        );
    }

    /// While typing in the URL bar, keystrokes must never flow to the terminal.
    /// If they did, typing what feels like a destination would actually send text to the AI.
    #[test]
    fn typing_an_address_does_not_reach_the_terminal() {
        assert!(PAGE.contains("e.stopPropagation();"), "打鍵を止めていない");
        // Selection or a click must never steal focus away from the input field
        assert!(
            PAGE.contains("if (a && a.closest && a.closest(\"#nav\")) return;"),
            "入力中に焦点を奪っている"
        );
        assert!(PAGE.contains("if (inBar(e)) return;"), "バーの中で端末の作法が働く");
    }

    /// The ball is a moving element, not a character.
    ///
    /// This is half the reason for having a window at all — a plain grid of cells would have made do with a ● character.
    #[test]
    fn the_ball_is_a_moving_thing_not_a_character() {
        assert!(PAGE.contains("#ball"), "ボールの要素が無い");
        assert!(PAGE.contains("transition:left"), "動かない");
        assert!(!PAGE.contains("\u{25CF}"), "文字の●で描いている");
    }
}

#[cfg(test)]
mod color_tests {
    use super::{PAGE, screen_html};

    fn render(input: &str) -> String {
        let mut p: vt100::Parser = vt100::Parser::new(3, 40, 0);
        p.process(input.as_bytes());
        screen_html(p.screen())
    }

    /// Every run must carry its own cell count.
    ///
    /// The terminal counts columns in cells; the browser lays text out
    /// using font advance widths. No font makes the two match exactly —
    /// Cascadia Mono's Latin characters are 0.586em wide, and full-width
    /// characters are 1.0em, which isn't exactly double. Even switching to
    /// a font that draws box-drawing characters in one cell, things drift
    /// out of alignment as soon as CJK text appears.
    ///
    /// Recording the cell count explicitly means the next run always
    /// starts from the right place, no matter what the font's advance
    /// width is. The font then only ever affects appearance.
    #[test]
    fn every_run_carries_its_own_width() {
        // A line mixing full-width characters, box-drawing characters, and Latin characters
        let html = render("\u{1b}[31mあ\u{1b}[0m\u{2502}ab");
        for piece in html.split("<span").skip(1) {
            assert!(
                piece.contains("width:calc(var(--cw)*"),
                "マス数を持たない区間がある: {piece}"
            );
        }
        // A full-width character is 2 cells
        assert!(
            html.contains("width:calc(var(--cw)*2)"),
            "全角が2マスになっていない: {html}"
        );
        // A run of 3 half-width characters is 3 cells
        let three = render("\u{1b}[31mabc\u{1b}[0m");
        assert!(
            three.contains("width:calc(var(--cw)*3)"),
            "半角3文字が3マスになっていない: {three}"
        );
        // Box-drawing characters are also 1 cell — the terminal counts them that way, so rendering matches
        let line = render("\u{1b}[31m\u{2502}\u{1b}[0m");
        assert!(
            line.contains("width:calc(var(--cw)*1)"),
            "罫線が1マスになっていない: {line}"
        );
    }

    /// Characters whose advance width doesn't match a cell get boxed individually.
    ///
    /// CJK advance width is narrower than 2 cells (it comes from a
    /// different font, so it doesn't line up). Merging them into a single
    /// box lets the shortfall accumulate at the end of the run — after 40
    /// characters, a gap of about 10 cells had opened up between the end
    /// of the string and the cursor.
    #[test]
    fn a_letter_that_does_not_fill_its_cell_gets_a_box_of_its_own() {
        let html = render("あいう");
        assert_eq!(
            html.matches("width:calc(var(--cw)*2)").count(),
            3,
            "全角がまとめられている: {html}"
        );
        assert!(html.contains("text-align:center;"), "マスの中で寄っている: {html}");

        // ASCII characters fit their cell exactly, so merging them is fine (avoids extra elements)
        let ascii = render("\u{1b}[31mabcdef\u{1b}[0m");
        assert_eq!(
            ascii.matches("width:calc(var(--cw)*").count(),
            1,
            "英数字まで1文字ずつ切っている: {ascii}"
        );
    }

    /// The content and the cursor must be placed using the same single number.
    ///
    /// Using separate numbers (ch for content, a measured value for the
    /// cursor) lets the difference between them compound column by column —
    /// the cursor drifted further right the more you typed.
    #[test]
    fn the_text_and_the_cursor_share_one_cell_width() {
        assert!(
            !render("ab").contains("ch;"),
            "フォントが言う字送りで桁を置いている"
        );
        assert!(
            PAGE.contains("scr.style.setProperty(\"--cw\", cellW + \"px\")"),
            "測った幅を中身へ渡していない"
        );
        // The cursor is also placed from that same cellW
        assert!(PAGE.contains("col * cellW"), "カーソルが別の数で置かれている");
    }

    /// Trailing whitespace at the end of a line doesn't need a fixed position.
    /// There's nothing after it, so boxing out 40 columns' worth on every line would be pointless.
    #[test]
    fn the_blank_tail_of_a_line_needs_no_box() {
        let html = render("ab");
        let first = html.lines().next().unwrap_or_default();
        assert!(first.starts_with("<span"), "{first}");
        assert!(
            first.trim_end().ends_with("</span>") || first.ends_with(' '),
            "行末の空白まで箱に入れている: {first:?}"
        );
    }

    /// Colors emitted by a program are rendered as-is.
    ///
    /// Before this, only plain text was sent, so build warnings, git
    /// diffs, and the AI's own emphasis all looked like the same shade of gray.
    #[test]
    fn colours_reach_the_screen() {
        // One of the sixteen names a variable and the theme decides what
        // that variable is: this is where a colour scheme reaches a cell
        let h = render("\x1b[31mred\x1b[0m plain");
        assert!(h.contains("color:var(--c1)"), "前景色が出ていない: {h}");
        assert!(h.contains(">red<"), "色の中身が入っていない: {h}");
        assert!(h.contains("plain"), "色なしの部分が消えている: {h}");

        // Background, bold, underline
        assert!(render("\x1b[44mx").contains("background:var(--c4)"), "背景色");
        assert!(render("\x1b[1mx").contains("font-weight:700"), "太字");
        assert!(render("\x1b[4mx").contains("text-decoration:underline"), "下線");

        // Inverse swaps the foreground and background
        let inv = render("\x1b[7mx");
        assert!(inv.contains("background:") && inv.contains("color:"), "反転: {inv}");

        // The 256-color cube and the grayscale ramp
        assert!(render("\x1b[38;5;196mx").contains("color:#ff0000"), "立方体の赤");
        assert!(render("\x1b[38;5;232mx").contains("color:#080808"), "灰色の下端");
        // 24-bit
        assert!(render("\x1b[38;2;18;52;86mx").contains("color:#123456"), "24bit色");
    }

    /// Characters shown on screen are never interpreted as HTML markup.
    ///
    /// It's completely ordinary for `<script>` to show up in a program's
    /// output (catting an HTML file, grep results, an AI's reply)
    #[test]
    fn output_is_never_treated_as_markup() {
        let h = render("<script>alert(1)</script> & <b>");
        assert!(!h.contains("<script>"), "生のタグが残っている: {h}");
        assert!(h.contains("&lt;script&gt;"), "エスケープされていない: {h}");
        assert!(h.contains("&amp;"), "アンパサンドが素通り: {h}");
    }

    /// Runs with the same appearance are merged into one.
    ///
    /// One element per cell would produce 9000 elements for a 50-row by
    /// 180-column screen, making every redraw more expensive.
    #[test]
    fn runs_of_the_same_look_are_merged() {
        let h = render("\x1b[31maaaaaaaaaa");
        assert_eq!(h.matches("<span").count(), 1, "文字ごとに分かれている: {h}");

        // A change in appearance splits the run
        let h = render("\x1b[31ma\x1b[32mb\x1b[31mc");
        assert_eq!(h.matches("<span").count(), 3, "{h}");
    }
}


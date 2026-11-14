/** The scissors' tools on the actual tool page: choosing one for a frame,
 * copying the framed part, and moving what was drawn in the editor.
 * node tools/debug/shoot.mjs tools/debug/scenes/snip-tools.mjs
 *
 * The page is driven the way a person drives it -- presses, drags and keys --
 * and each scene throws when what came of them is not what the page promises.
 * The window's clipboard is not here, so its answer (`__snipCopied`) is given
 * by the scene.
 */
const START = `
  const check = (ok, message) => { if (!ok) throw new Error(message); };
  // A press made here is not one the browser is tracking
  Element.prototype.setPointerCapture = function () {};
  const frame = () => new Promise(done => requestAnimationFrame(() => requestAnimationFrame(done)));
  const press = (el, type, x, y) => el.dispatchEvent(new PointerEvent(type,
    {clientX: x, clientY: y, button: 0, pointerId: 1, bubbles: true, pointerType: "mouse"}));
  const key = k => document.dispatchEvent(new KeyboardEvent("keydown", {key: k, bubbles: true}));
  // A picture with something in it to frame and to draw over
  const picture = () => new Promise(done => {
    const c = document.createElement("canvas");
    c.width = 1280; c.height = 800;
    const g = c.getContext("2d");
    g.fillStyle = "#20242c"; g.fillRect(0, 0, 1280, 800);
    g.fillStyle = "#2f6fed"; g.fillRect(80, 80, 420, 240);
    g.fillStyle = "#e9eef7"; g.fillRect(560, 120, 600, 36); g.fillRect(560, 190, 480, 36); g.fillRect(560, 260, 540, 36);
    g.fillStyle = "#30a46c"; g.fillRect(80, 420, 1080, 300);
    const i = new Image();
    i.onload = () => done(i);
    i.src = c.toDataURL("image/png");
  });
  const stage = $("stage");
  const framed = async (x1, y1, x2, y2) => {
    begin(await picture());
    press(stage, "pointerdown", x1, y1);
    press(stage, "pointermove", x2, y2);
    press(stage, "pointerup", x2, y2);
  };
`;

export default {
  page: 'snip',
  scenes: {
    // A frame drawn, and the tools offered for it: copying first, chosen by
    // Enter, with its letter beside it
    'snip-choose': `(async () => {${START}
      await framed(innerWidth * 0.15, 150, innerWidth * 0.55, 380);
      const rows = [...$("choose").querySelectorAll("button")];
      check(!$("choose").hidden && rows.length === TOOLS.length, "the tools are not offered for the frame");
      check(rows[0].querySelector("span").textContent === T["snip.tool.clip"], "copying is not the first tool");
      check(rows[0].querySelector("kbd").textContent === "X", "copying does not show its letter");
      check(document.activeElement === rows[0], "Enter would not choose the first tool");
      check(rows.every(b => !b.classList.contains("off")), "a tool is grey where it can run");
      const box = $("choose").getBoundingClientRect();
      check(box.left >= 0 && box.right <= innerWidth && box.bottom <= innerHeight, "the list is off the screen");
    })()`,

    // X hands the framed part to the clipboard; the page says so only once
    // the window has answered, and then it is on its way out
    'snip-clip-copied': `(async () => {${START}
      await framed(innerWidth * 0.15, 150, innerWidth * 0.55, 380);
      key("x");
      await frame(); await frame();
      check(leaving === "copying" && $("choose").hidden, "X did not start the copy");
      check(!$("toast").classList.contains("show"), "it said copied before the clipboard answered");
      check(pictureCopied.size === 1, "the window was not handed the picture");
      // An answer to some other copy is not this one's
      window.__snipCopied(copies + 7, true);
      await frame();
      check(leaving === "copying", "an answer to another copy was taken for this one");
      window.__snipCopied(copies, true);
      await frame();
      check(leaving === "copied", "the page did not hear the clipboard's answer");
      check($("toast").classList.contains("show") && !$("toast").classList.contains("warn")
        && $("toastmsg").textContent === T["snip.edit.copied"], "it did not say the part was copied");
      check(!$("frame").hidden && !$("stage").hidden, "the frame went before the page did");
    })()`,

    // The clipboard would not take it: the frame stays, the tools are offered
    // again and the reason is under them
    'snip-clip-refused': `(async () => {${START}
      await framed(innerWidth * 0.15, 150, innerWidth * 0.55, 380);
      key("x");
      await frame(); await frame();
      window.__snipCopied(copies, false);
      await frame();
      const why = $("choose").querySelector(".why");
      check(leaving === "" && !$("choose").hidden, "the tools were not offered again");
      check(!why.hidden && why.textContent === T["snip.clip.refused"], "the reason is not said under the tools");
      check(!$("toast").classList.contains("show"), "it said copied though the clipboard refused");
      check(pending && Math.round(pending.b.x - pending.a.x) === rect.w, "the frame offered again is not the one that was drawn");
      const box = $("choose").getBoundingClientRect();
      check(box.left >= 0 && box.right <= innerWidth && box.bottom <= innerHeight, "the list is off the screen");
    })()`,

    // A press that does not move, with the tool chosen first: the whole picture
    'snip-clip-whole': `(async () => {${START}
      begin(await picture());
      key("x");
      check($("hint").textContent.includes(T["snip.tool.clip"]), "the hint does not say which tool was chosen");
      press(stage, "pointerdown", innerWidth / 2, innerHeight / 2);
      press(stage, "pointerup", innerWidth / 2, innerHeight / 2);
      await frame(); await frame();
      check(rect.x === 0 && rect.y === 0 && rect.w === 1280 && rect.h === 800, "a press that did not move did not take the whole picture");
      window.__snipCopied(copies, true);
      await frame();
      check(leaving === "copied", "the whole picture was not copied");
    })()`,

    // The editor: something of every kind drawn, then picked up with the hand
    // and put elsewhere, undone and done again
    'snip-edit-move': `(async () => {${START}
      begin(await picture());
      TOOL = "edit";
      openTool({x: 0, y: 0}, {x: 1280, y: 800});
      const area = $("e-area");
      // A point of the picture, as a point of the screen
      const on = (x, y) => { const r = area.getBoundingClientRect(); return [r.left + E.view.ox + x * E.view.s, r.top + E.view.oy + y * E.view.s]; };
      const drag = (x1, y1, x2, y2) => {
        press(area, "pointerdown", ...on(x1, y1));
        press(area, "pointermove", ...on((x1 + x2) / 2, (y1 + y2) / 2));
        press(area, "pointermove", ...on(x2, y2));
        press(area, "pointerup", ...on(x2, y2));
      };
      const rail = [...$("e-tools").querySelectorAll("button")];
      check(rail[0].querySelector("span").textContent === T["snip.edit.tool.move"] && rail[0].querySelector("svg"), "the hand is not at the top of the rail");
      check(rail[1].querySelector("span").textContent === T["snip.edit.tool.arrow"], "the hand is not above the arrow");

      // With nothing drawn the hand says so, and a press does nothing else
      key("v");
      check(E.tool === "move" && area.classList.contains("moving"), "V does not choose the hand");
      check($("e-opts").children.length === 0, "the hand offers colours it cannot use");
      press(area, "pointerdown", ...on(300, 300)); press(area, "pointerup", ...on(300, 300));
      check($("e-say").textContent === T["snip.edit.move.none"] && !E.undo.length, "a press on nothing did not say why");

      editTool("arrow"); drag(620, 520, 300, 240);
      editTool("rect"); drag(540, 100, 1180, 320);
      editTool("hide"); drag(560, 180, 1040, 240);
      editTool("pen"); drag(120, 600, 520, 680);
      editTool("text");
      press(area, "pointerdown", ...on(700, 480));
      $("e-text").value = "Moved here";
      editEndText();
      check(E.objs.length === 5, "the five things were not drawn (" + E.objs.length + ")");
      const steps = E.undo.length;
      const [arrow, box, hidden, line, words] = E.objs;

      key("v");
      // Over the arrow's line: outlined, and the pointer is an open hand
      press(area, "pointermove", ...on(460, 380));
      check(E.over === arrow && area.classList.contains("over"), "the arrow under the pointer is not offered");
      // Inside the box is the picture, not the box
      press(area, "pointermove", ...on(860, 150));
      check(E.over === null && !area.classList.contains("over"), "the inside of a box picks the box up");
      press(area, "pointermove", ...on(540, 150));
      check(E.over === box, "the edge of a box does not pick it up");
      press(area, "pointermove", ...on(800, 210));
      check(E.over === hidden, "a hidden part is not picked up by its inside");
      press(area, "pointermove", ...on(320, 640));
      check(E.over === line, "a pen line is not picked up");
      press(area, "pointermove", ...on(760, 500));
      check(E.over === words, "words are not picked up");

      // The arrow, 200 right and 100 down
      press(area, "pointerdown", ...on(460, 380));
      check(E.moving && area.classList.contains("held"), "a press on the arrow did not pick it up");
      press(area, "pointermove", ...on(560, 430));
      press(area, "pointermove", ...on(660, 480));
      await frame();
      press(area, "pointerup", ...on(660, 480));
      const near = (a, b) => Math.abs(a - b) < 0.01;
      const now = E.objs[0];
      check(E.objs.length === 5 && now !== arrow, "the arrow was changed where it stood");
      check(near(now.x1, arrow.x1 + 200) && near(now.y1, arrow.y1 + 100) && near(now.x2, arrow.x2 + 200) && near(now.y2, arrow.y2 + 100),
        "the arrow is not where it was put");
      check(E.objs[1] === box && E.objs[4] === words, "moving one thing changed the order of the others");
      check(E.undo.length === steps + 1 && E.dirty, "the move is not a step that can be undone");
      check(near(arrow.x1, 620) && near(arrow.y1, 520), "the arrow kept for undo was moved with it");

      // The words, then back with undo, forward with redo
      press(area, "pointerdown", ...on(760, 500));
      press(area, "pointermove", ...on(260, 120));
      await frame();
      press(area, "pointerup", ...on(260, 120));
      check(near(E.objs[4].x, words.x - 500) && near(E.objs[4].y, words.y - 380) && E.objs[4].text === "Moved here", "the words are not where they were put");
      const put = E.objs[4];
      key("z");
      check(E.objs[4] === put, "a plain Z undid a move");
      document.dispatchEvent(new KeyboardEvent("keydown", {key: "z", ctrlKey: true, bubbles: true}));
      check(E.objs[4] === words && E.objs[0] === now, "undo did not put the words back");
      document.dispatchEvent(new KeyboardEvent("keydown", {key: "y", ctrlKey: true, bubbles: true}));
      check(near(E.objs[4].x, words.x - 500), "redo did not move the words again");

      // Esc while holding puts it back, and is not a step
      const before = E.undo.length;
      press(area, "pointerdown", ...on(540, 150));
      press(area, "pointermove", ...on(700, 400));
      // A second finger meanwhile takes nothing over
      area.dispatchEvent(new PointerEvent("pointerdown", {clientX: on(320, 640)[0], clientY: on(320, 640)[1], button: 0, pointerId: 2, bubbles: true, pointerType: "touch"}));
      check(E.moving && E.moving.obj === box, "a second press took over the thing being held");
      key("Escape");
      check(!E.moving && E.objs[1] === box && E.undo.length === before && !$("edit").hidden, "Esc while holding did not put the box back");
      check(!area.classList.contains("over") && !area.classList.contains("held"), "the pointer still says something is held");
      // Cutting is a cross, and the hand is back after it
      editCropStart();
      check(!area.classList.contains("moving"), "the hand's pointer stays while choosing where to cut");
      editCropEnd();
      check(area.classList.contains("moving"), "the hand's pointer did not come back after cutting");
      press(area, "pointerup", ...on(700, 400));

      // A press that does not move is not a step either
      press(area, "pointerdown", ...on(540, 150)); press(area, "pointerup", ...on(540, 150));
      check(E.undo.length === before, "a press that did not move was kept as a step");

      // Dragged past the edge, the held point stays on the picture
      press(area, "pointerdown", ...on(320, 640));
      press(area, "pointermove", ...on(-900, -900));
      await frame();
      press(area, "pointerup", ...on(-900, -900));
      const far = E.objs[3];
      check(far.pts.some(p => p.x >= 0 && p.x <= 1280 && p.y >= 0 && p.y <= 800), "a thing dragged past the edge is gone for good");
      document.dispatchEvent(new KeyboardEvent("keydown", {key: "z", ctrlKey: true, bubbles: true}));

      // Left for the photograph: the hand over the moved arrow
      press(area, "pointermove", ...on(660, 480));
      check(E.over === E.objs[0], "the moved arrow is not offered where it now is");
      await frame();
      check(document.documentElement.scrollWidth <= innerWidth, "the editor is wider than the screen");
    })()`,
  },
};

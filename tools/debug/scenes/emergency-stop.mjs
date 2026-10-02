/**
 * The emergency-stop confirmation, on the window and remote board.
 * Run: node tools/debug/shoot.mjs tools/debug/scenes/emergency-stop.mjs
 * Needs cargo and Chrome, as shoot.mjs does. Requests stay in the page:
 * check every way of cancelling and confirming before taking the picture.
 */
const run = `
  (async () => {
    const sent = [];
    window.ipc.postMessage = text => sent.push(JSON.parse(text));
    window.fetch = async (url, options = {}) => {
      if (options.method === "POST") sent.push(JSON.parse(options.body));
      return new Response("{}", {headers:{"Content-Type":"application/json"}});
    };
    S = {tabs:[], active:0, desk:"Workspace", auto_enabled:true};
    drawStatus();
    document.getElementById("splash").hidden = true;
    const box = document.getElementById("sask");
    const check = (ok, why) => { if (!ok) throw Error(why); };
    const open = async () => {
      document.getElementById("stop").click();
      await new Promise(resolve => setTimeout(resolve, 0));
      check(!box.hidden, "Stop did not ask for confirmation");
      check(sent.length === 0, "Opening the question sent a request");
      check(box.querySelector(".vtitle").textContent === T["tui.stop.confirm"], "Missing question");
      check(box.querySelector(".vsay").textContent === T["tui.stop.say"], "Missing scope");
      check(document.activeElement === box.querySelector(".go"), "Confirmation has no keyboard focus");
      drawStatus();
      check(!box.hidden, "A status update dismissed the question");
    };
    for (const cancel of [
      () => box.querySelector(".quiet").click(),
      () => box.querySelector(".vclose").click(),
      () => box.dispatchEvent(new MouseEvent("mousedown", {bubbles:true})),
      () => document.activeElement.dispatchEvent(new KeyboardEvent("keydown", {key:"Escape", bubbles:true})),
    ]) {
      await open();
      cancel();
      check(box.hidden && sent.length === 0, "Cancelling sent a request or left the question open");
    }
    for (const confirm of [
      () => box.querySelector(".go").click(),
      () => document.activeElement.dispatchEvent(new KeyboardEvent("keydown", {key:"Enter", bubbles:true})),
    ]) {
      await open();
      confirm();
      check(box.hidden && sent.length === 1 && sent[0].kind === "stop", "Confirmation did not send one stop");
      sent.length = 0;
    }
    await open();
    return "Stop asks every time; cancellation sends nothing; confirmation sends one stop";
  })()
`;

export default {
  scenes: {
    'emergency-stop': {run, sizes:[['wide', 1280, 860]]},
    'emergency-stop-remote': {run, served:'remote', sizes:[['phone', 390, 820]]},
  },
};

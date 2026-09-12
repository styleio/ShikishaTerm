// Exercise the real settings handlers with disposable records. Every request
// stays here; a test must never revoke a real device or remove a real secret.
window.probeWrites = [];
window.probeSecrets = [];
window.fetch = async (url, options = {}) => {
  if (options.method && options.method !== "GET") probeWrites.push(String(url));
  let value = {ok:true};
  if (url === "/api/config") value = {workspaces:[]};
  else if (url === "/api/secrets") value = {mode:"encrypted", secrets:probeSecrets};
  else if (url === "/api/secrets/orphans") value = {orphans:["unused"]};
  else if (url === "/api/remote/clients") value = {clients:[{id:"test-device", name:"Travel phone", seen:0}]};
  else if (String(url).startsWith("/api/family")) value = {cut:true};
  return {ok:true, status:200, json:async () => value, text:async () => JSON.stringify(value)};
};
window.probeNative = [];
const originalConfirm = window.confirm;
window.confirm = message => {
  const answer = originalConfirm(message);
  probeNative.push(answer);
  return answer;
};
window.probePrepare = kind => {
  document.querySelectorAll(".modal, dialog").forEach(e => e.remove());
  probeWrites = []; probeSecrets = []; probeNative = [];
  current = {providers:{sample:{base_url:"https://example.com", api_key:"@provider/sample"}},
    notify:{sample:{type:"telegram", token:"@notify/sample"}}};
  const ws = {id:"sample", name:"Sample workspace", folders:[{cwd:""},{cwd:"branch",name:"Shipping labels"}],
    tabs:[newTab({id:"sample",name:"Sample tab",command:"cmd.exe",group:0})]};
  wss = [ws]; sel = {ws:0,tab:null,global:false}; loadFailure = null;
  const host = document.getElementById("detail");
  host.replaceChildren();
  let key;
  const add = child => host.append(child);
  if (kind === "device") { add(remoteCard()); key = "settings.phone.device.revoke"; }
  if (kind === "orphans") { add(filesCard()); key = "settings.secrets.orphans_clean"; }
  if (kind === "provider") { providerDialog("sample", () => {}); key = "settings.providers.delete"; }
  if (kind === "notify") { chatDialog("sample", () => {}); key = "settings.notify.delete"; }
  if (kind === "workspace" || kind === "workspace-secrets") {
    if (kind === "workspace-secrets") probeSecrets = [{key:"sample.password"}];
    add(wsPane(ws)); key = "settings.workspace.delete";
  }
  if (kind === "folder") { add(folderPane(ws, ws.folders[1], 1)); key = "settings.group.discard"; }
  if (kind === "secret") {
    secretDialog(ws, {key:"sample.password",short:"password",urls:[]}); key = "common.delete";
  }
  if (kind === "tab") { sel.tab = 0; add(tabPane(ws, ws.tabs[0])); key = "settings.tab.delete"; }
  if (kind === "close") {
    savedSnapshot = "changed";
    add(el("button", {onclick:closeSettings}, T["settings.back"] || T["common.close"]));
    key = T["settings.back"] ? "settings.back" : "common.close";
  }
  window.probeKey = key;
  window.probeState = () => JSON.stringify({writes:probeWrites, providers:current.providers,
    notify:current.notify, workspaces:wss.length, tabs:ws.tabs.length, folders:ws.folders.length});
  return key;
};
window.probeButton = (key, selector = "button") => {
  const buttons = [...document.querySelectorAll(selector)];
  const button = buttons.find(e => e.textContent.trim() === T[key]);
  if (!button) throw Error("Missing button: " + key);
  button.scrollIntoView({block:"center"});
  const r = button.getBoundingClientRect();
  return {x:(r.x + r.width/2)*devicePixelRatio,y:(r.y+r.height/2)*devicePixelRatio};
};

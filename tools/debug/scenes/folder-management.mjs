/**
 * Check folder selection, confirmation and live updates on the real board page.
 * Run: node tools/debug/shoot.mjs tools/debug/scenes/folder-management.mjs
 * Requires cargo and Chrome. All outgoing requests are captured in the page.
 */
const run = `
(async () => {
  const check = (ok, why) => { if (!ok) throw Error(why); };
  const sent = [];
  window.ipc.postMessage = text => sent.push(JSON.parse(text));
  window.fetch = async (url, options = {}) => {
    if (options.method === "POST") sent.push(JSON.parse(options.body));
    return new Response("{}", {headers:{"Content-Type":"application/json"}});
  };
  const groups = [
    {key:"D:/work/website", folder:"D:/work/website", name:"Website", keep_first:true},
    {key:"D:/work/login", folder:"D:/work/login", name:"Login flow", linked:true},
    {key:"D:/work/search", folder:"D:/work/search", name:"Search results", linked:true},
    {key:"D:/work/old-layout", folder:"D:/work/old-layout", name:"Earlier layout", parked:true, linked:true},
  ].map(g => ({health:{as:"fine"}, drift:{}, project:"Website", family:"D:/work/website/.git", ...g}));
  const measured = 1790996400;
  S = {desk:"Website", desk_uid:"workspace-a", groups, tabs:[], active:0,
    folder_manage:{usage:{
      [groups[0].key]:{bytes:893427712, measured},
      [groups[1].key]:{bytes:251658240, measured},
      [groups[2].key]:{bytes:16384, measured, partial:true, error:T["err.folders.measure_timeout"]},
    }, results:{[groups[0].key]:{error:T["err.folders.pinned"]},
      [groups[1].key]:{error:T["err.folders.shared"]}}}};
  document.getElementById("splash").hidden = true;
  drawTabs();
  check(!document.getElementById("tabs").textContent.includes("Earlier layout"), "Archived folder leaked into the board");
  check(document.getElementById("tabs").textContent.includes(T["tui.folders.manage"]), "Manager is not discoverable");
  openFolderManager();
  const box = document.getElementById("sask");
  const m = folderManager;
  m.action.click();
  check(!m.why.hidden && m.action.classList.contains("held"), "Empty selection has no persistent explanation");
  check(m.all.getBoundingClientRect().width > 0, "Select-visible checkbox is hidden");
  m.all.click();
  check(m.selected.size === 3, "Select-visible did not select the active list");
  m.all.click();
  check(m.visible.length === 3, "Active filter includes archived folders");
  const search = box.querySelector("input[type=search]");
  const first = m.rows.get(groups[1].key).check;
  first.click(); first.focus();
  drawFolderManager();
  check(first === m.rows.get(groups[1].key).check && document.activeElement === first && first.checked,
    "Live update replaced the selected/focused checkbox");
  search.value = "Search"; search.dispatchEvent(new Event("input"));
  check(m.selected.size === 0 && m.visible.length === 1, "Filter left an invisible selection");
  search.value = ""; search.dispatchEvent(new Event("input"));
  m.selected.add(groups[1].key); m.selected.add(groups[2].key); drawFolderManager();
  folderAction("delete", groups.slice(1,3), () => openFolderManager(m));
  check(sent.length === 0 && box.querySelector(".blist").textContent.includes(groups[1].folder), "Delete skipped its target confirmation");
  closeAsk();
  check(sent.length === 0 && folderManager === m && !m.question, "Cancel lost the selection or sent deletion");
  folderAction("delete", groups.slice(1,3), () => openFolderManager(m));
  box.querySelector(".go").click();
  check(sent.length === 1 && sent[0].desk === "workspace-a" && sent[0].folders.length === 2 && sent[0].act === "delete", "Confirmation did not send exactly the selected folders");
  sent.length = 0;
  folderAction("archive", [groups[1]], () => openFolderManager(m));
  closeAsk();
  check(!sent.length, "Cancelling archive sent a request");
  m.filter = "archived"; drawFolderManager();
  check(m.visible.length === 1 && !m.selected.size, "Archive filter kept hidden selection");
  folderAction("restore", m.visible);
  check(sent.pop()?.act === "restore", "Restore did not send a request");
  m.filter = "active"; drawFolderManager();
  m.measure.click();
  check(sent.pop()?.act === "measure", "Capacity request is not explicit");
  folderAction("delete", [groups[1]], () => openFolderManager(m));
  S.desk_uid = "workspace-b"; drawFolderManager();
  check(box.hidden && !folderManager, "Desk switch retained a destructive confirmation");
  S.desk_uid = "workspace-a";
  openFolderManager();
  folderManager.selected.add(groups[1].key); folderManager.selected.add(groups[2].key);
  drawFolderManager();
  await new Promise(resolve => setTimeout(resolve, 0));
  check(document.activeElement === box.querySelector("input[type=search]"), "Manager does not focus its first field");
  const rect = box.querySelector(".vbox").getBoundingClientRect();
  check(rect.left >= 0 && rect.right <= innerWidth && rect.bottom <= innerHeight, "Manager does not fit the viewport");
  return "Selection, cancellation, confirmation, restore, measurement and desk isolation passed";
})()
`;

export default {
  scenes: {
    'folder-management': {run, sizes:[['wide',1280,860]]},
    'folder-management-remote': {run, served:'remote', sizes:[['phone',390,820]]},
  },
};

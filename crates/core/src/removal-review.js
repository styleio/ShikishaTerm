// Both pages ask about the same files and carry the same explicit answer.
// Skipping a clean-folder question never approves discarding file changes.
async function reviewFolderRemoval(groups, {request, question, skip = false, openFile}) {
  const checks = [];
  for (const group of groups) {
    const answer = await request("/api/folder/discard-check", {folder:group.key});
    if (!answer || !answer.ok) throw new Error(answer && answer.error || T["settings.group.discard.failed"]);
    checks.push({group, ...answer});
  }
  const blocked = checks.find(c => c.blocked);
  if (blocked) throw new Error(blocked.group.folder + "\n" + blocked.blocked);
  const count = checks.reduce((n, c) => n + c.files.length, 0);
  const reviews = Object.fromEntries(checks.map(c => [c.group.key, c.review]));
  if (skip && !count && groups.length === 1) return {reviews, unasked:false};
  const rows = [];
  for (const c of checks) {
    rows.push(el("div", {class:"brow2 stacked"}, el("span", {class:"nm asis"}, c.group.folder),
      el("span", {class:"tag"}, c.keeps_branch
        ? T["tui.discard.branch"].replaceAll("{branch}", c.branch) : T["tui.discard.history_lost"])));
    for (const file of c.files) {
      const controls = [];
      if (openFile && c.board !== false) {
        const view = (label, diff) => el("button", {type:"button", class:"quiet", onclick:() => openFile(c.group, file, diff)}, label);
        if (file.staged) controls.push(view(T["tui.discard.view_staged"], "staged"));
        if (file.work) controls.push(view(T["tui.discard.view"], file.untracked ? "" : "work"));
      }
      rows.push(el("div", {class:"brow2 stacked"},
        el("span", {class:"nm asis"}, file.path),
        el("span", {class:"tag"}, T[file.untracked ? "tui.discard.untracked" : "tui.discard.modified"]), ...controls));
    }
  }
  const answer = await question({rows, dirty:count > 0,
    say:count ? T["tui.discard.changes"].replaceAll("{n}", count) : T["tui.discard.say"],
    label:T[count ? "tui.discard.changes_go" : "tui.discard.go"]});
  return answer ? {reviews, unasked:!count && !!answer.unasked} : null;
}

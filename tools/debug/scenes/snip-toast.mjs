/** Shared notifications on the actual picture tools page.
 * node tools/debug/shoot.mjs tools/debug/scenes/snip-toast.mjs
 */
export default {
  page: 'snip',
  scenes: {
    'snip-toast': `(async () => {
      const check = (ok, message) => { if (!ok) throw new Error(message); };
      offerPick();
      toast(T['snip.failed'], true);
      const box = document.getElementById('toast');
      check(box.classList.contains('show') && box.classList.contains('warn'), 'Warning did not show');
      check(document.getElementById('toastmsg').textContent === T['snip.failed'], 'Warning text missing');
      check(toastMs(T['snip.failed'], true) >= 6000, 'Warning disappears too quickly');
      box.click();
      check(!box.classList.contains('show'), 'Warning cannot be dismissed');
      toast(T['snip.copied']);
      check(!box.classList.contains('warn'), 'Success kept warning appearance');
      check(document.documentElement.scrollWidth <= innerWidth, 'Toast overflows the screen');
    })()`,
  },
};

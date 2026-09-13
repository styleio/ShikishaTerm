// A click on a "get it from the Store" link opens the Store in a new tab —
// and this window stays with us, moving to the page that says thanks.
//
// The Store cannot be started the way the zip can (it is a navigation, not a
// download), so without this the moment someone decides to install is the
// moment our site goes quiet. With JS off, only the first half happens, which
// is exactly the behaviour we had before.
document.addEventListener("click", function (ev) {
  if (ev.defaultPrevented || ev.button !== 0) return;
  if (ev.metaKey || ev.ctrlKey || ev.shiftKey || ev.altKey) return;

  var link = ev.target.closest && ev.target.closest('a[href*="apps.microsoft.com"]');
  if (!link) return;

  var thanks = document.documentElement.lang === "ja" ? "/ja/store/" : "/store/";
  // On the thanks page itself the same link is the way out when the Store did
  // not open. Sending it back to where it already is would strand people.
  if (location.pathname.replace(/\/?$/, "/") === thanks) return;

  // Not every Store link opens a tab of its own: the one in the steps, and
  // the "next page" link the sidebar makes, open in this one. Moving this
  // window to the thanks page then cancelled the Store before it loaded, and
  // the visitor was thanked for a Store that never opened. Such a link is
  // opened in a tab here instead; if the browser will not allow one, the link
  // is left to go where it says.
  if (link.target !== "_blank") {
    var store = window.open(link.href, "_blank");
    if (!store) return;
    try { store.opener = null; } catch (e) {}
    ev.preventDefault();
  }

  setTimeout(function () {
    location.href = thanks;
  }, 0);
});

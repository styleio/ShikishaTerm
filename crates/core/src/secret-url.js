const SECRET_URL_POLICY = {{SECRET_URL_POLICY}};
const SHARED_SUFFIX = new Set(SECRET_URL_POLICY.suffixes);
// Why this line cannot be used, as the name of the sentence to show, or null.
// The same answers, in the same order, as config.rs's url_fault
function urlFault(text) {
  const t = (text || "").replace(/^\p{White_Space}+|\p{White_Space}+$/gu, "");
  if (!t) return "err.secret_url.empty";
  if (/\p{White_Space}/u.test(t)) return "err.secret_url.unreadable";
  const at = t.indexOf("://");
  if (at < 0) return "err.secret_url.scheme";
  const scheme = t.slice(0, at).toLowerCase();
  if (!SECRET_URL_POLICY.schemes.includes(scheme)) return "err.secret_url.scheme";
  const host = t.slice(at + 3).split(/[/?#]/)[0].toLowerCase();
  if (!host || host.includes("@")) return "err.secret_url.unreadable";
  const stars = (host.match(/\*/g) || []).length;
  if (stars) {
    if (stars > 1 || !host.startsWith("*.")) return "err.secret_url.star_place";
    const under = host.slice(2).split(":")[0];
    if (under.split(".").length < 2 || SHARED_SUFFIX.has(under)) return "err.secret_url.star_wide";
  }
  if (host.includes("[")) return "err.secret_url.unreadable";
  const colon = host.lastIndexOf(":");
  if (colon >= 0) {
    const port = host.slice(colon + 1);
    if (!host.slice(0, colon) || !/^\+?\d+$/.test(port) || Number(port) > SECRET_URL_POLICY.maxPort)
      return "err.secret_url.unreadable";
  }
  return null;
}

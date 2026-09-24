// Visitor counting for the site: Cloudflare Web Analytics.
//
// Chosen because it sets no cookie and stores nothing in the browser, so the
// pages need no consent banner (2026-09-24). The token is not a secret -- it is
// printed into every page's HTML -- so it lives here in the open, where a reader
// of the repository can see exactly what runs. Empty token = no script at all,
// which is what a local build and a fork get.
//
// Both heads import this: Starlight's (astro.config.mjs) and the landing page's
// own (LandingPage.astro). One place to change, so the two never drift.
//
// To get a token: Cloudflare dashboard > Analytics & Logs > Web Analytics >
// Add a site > shikisha-term.com, with the manual (JS snippet) setup. Do not
// also turn on Cloudflare's automatic injection, or every page counts twice.
export const CF_BEACON_TOKEN = "";

/** Attributes of the one <script> tag, or null when there is nothing to load. */
export const beacon = CF_BEACON_TOKEN
  ? {
      src: "https://static.cloudflareinsights.com/beacon.min.js",
      "data-cf-beacon": JSON.stringify({ token: CF_BEACON_TOKEN }),
      defer: true,
    }
  : null;

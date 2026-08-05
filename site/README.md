# Website

The landing page and documentation for ShikishaTerm-AI, built with
[Astro Starlight](https://starlight.astro.build/) and hosted on Cloudflare Pages.

## The point of this setup

**The documentation is not duplicated here.** `docs/AUTOMATION.md` in the repository root is
the single source: it is embedded in the `.exe` with `include_str!`, it is the specification
handed to the AI by the "let an AI write it" button, *and* it is a page on this site.

`npm run sync` regenerates the site pages from `docs/` before every build, so publishing a
documentation change is just editing the markdown and merging. The generated files are
gitignored — never edit them.

| Site page | Comes from |
|---|---|
| `/automation/` | `docs/AUTOMATION.md` |
| `/translating/` | `docs/TRANSLATING.md` |
| `/ja/automation/` | `docs/AUTOMATION.ja.md` |
| `/` and `/ja/` | `site/src/content/docs/index.mdx`, `ja/index.mdx` (hand-written) |

## Local development

```
cd site
npm install
npm run dev      # http://localhost:4321
npm run build    # output in dist/
```

## Deploying to Cloudflare Pages

Deployment runs from GitHub Actions (`.github/workflows/site.yml`) on every push to `main`
that touches `site/` or `docs/` — `docs/` matters because the pages are generated from it.

### One-time setup

1. Create a Pages project in the Cloudflare dashboard (Workers &amp; Pages → Create →
   Pages → **Direct Upload**). The workflow pushes the built site to it; it does not need
   Cloudflare's Git integration. Note the project name.
2. Create an API token at <https://dash.cloudflare.com/profile/api-tokens> →
   **Create Custom Token**, with just **Account / Cloudflare Pages / Edit**.
3. Add these under Settings → Secrets and variables → Actions:

   | Kind | Name | Value |
   |---|---|---|
   | Secret | `CLOUDFLARE_API_TOKEN` | the token from step 2 |
   | Secret | `CLOUDFLARE_ACCOUNT_ID` | shown in the Cloudflare sidebar |
   | Variable | `CLOUDFLARE_PROJECT_NAME` | the project name from step 1 (defaults to `shikishaterm`) |

After attaching a custom domain, update `site` in `astro.config.mjs` — it is used for the
sitemap and for social preview URLs.

### Or skip the workflow entirely

Cloudflare's own Git integration needs no tokens: connect the repository in the dashboard
with root directory `site`, build command `npm run build`, output directory `dist`. Delete
`.github/workflows/site.yml` if you go that way, so the site is not deployed twice.

## Adding a language to the site

1. Add the locale to `locales` in `astro.config.mjs`
2. Add `src/content/docs/<code>/index.mdx` for the landing page
3. If a translated manual exists as `docs/AUTOMATION.<code>.md`, add it to `PAGES` in
   `scripts/sync-docs.mjs`

Interface translations for the app itself are a separate, easier job — see
[docs/TRANSLATING.md](../docs/TRANSLATING.md).

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

Connected through Cloudflare's own Git integration, so there are no tokens to manage:
every push to `main` rebuilds and publishes.

| Setting | Value |
|---|---|
| Framework preset | Astro |
| Build command | `npm run build` |
| Build output directory | `dist` |
| Root directory | `site` |
| Environment variable | `NODE_VERSION` = `22` |

`NODE_VERSION` matters: Cloudflare's default Node is older than Astro needs, and that is
the usual reason a first build fails.

The build runs `npm run sync` first, which reads `docs/` from the repository root — one
level above the root directory set above. Editing the manual therefore republishes the
site, which is the point.

After attaching a custom domain, update `site` in `astro.config.mjs` — it is used for the
sitemap and for social preview URLs.

## Adding a language to the site

1. Add the locale to `locales` in `astro.config.mjs`
2. Add `src/content/docs/<code>/index.mdx` for the landing page
3. If a translated manual exists as `docs/AUTOMATION.<code>.md`, add it to `PAGES` in
   `scripts/sync-docs.mjs`

Interface translations for the app itself are a separate, easier job — see
[docs/TRANSLATING.md](../docs/TRANSLATING.md).

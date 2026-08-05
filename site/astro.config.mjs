import { defineConfig } from "astro/config";
import starlight from "@astrojs/starlight";

const REPO = "https://github.com/styleio/ShikishaTerm";

export default defineConfig({
  // sitemap と OGP のURLに使われるので、公開しているドメインと一致させる
  site: "https://shikisha-term.com",
  integrations: [
    starlight({
      title: "ShikishaTerm-AI",
      description:
        "Run Claude Code, Codex and Gemini side by side — and let them hand work to each other.",
      // 英語を根っこに置き、日本語を /ja/ に置く。lang/ の構成と同じ考え方
      defaultLocale: "root",
      locales: {
        root: { label: "English", lang: "en" },
        ja: { label: "日本語", lang: "ja" },
      },
      social: [{ icon: "github", label: "GitHub", href: REPO }],
      // 各ページに「編集する」リンクが出る。翻訳PRの入り口になる
      editLink: { baseUrl: `${REPO}/edit/main/site/` },
      lastUpdated: true,
      sidebar: [
        { label: "Automation", link: "/automation/" },
        { label: "Translating", link: "/translating/" },
        { label: "Download", link: `${REPO}/releases/latest`, attrs: { target: "_blank" } },
      ],
      customCss: ["./src/styles/custom.css"],
    }),
  ],
});

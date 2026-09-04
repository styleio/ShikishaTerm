import { defineConfig } from "astro/config";
import starlight from "@astrojs/starlight";

const REPO = "https://github.com/styleio/ShikishaTerm";

export default defineConfig({
  // sitemap と OGP のURLに使われるので、公開しているドメインと一致させる
  site: "https://shikisha-term.com",
  integrations: [
    starlight({
      title: "SHIKISHA-TERM",
      description:
        "Run Claude Code, Codex and Gemini side by side — and let them hand work to each other.",
      // 英語を根っこに置き、日本語を /ja/ に置く。lang/ の構成と同じ考え方
      defaultLocale: "root",
      locales: {
        root: { label: "English", lang: "en" },
        ja: { label: "日本語", lang: "ja" },
      },
      // 見出しに出るのは高さ32px程度なので、文字入りのロゴではなくアプリと同じ
      // アイコン (assets/icon.ico と同じ絵)。題字は replacesTitle: false のまま横に添える。
      //
      // ファイル名が mark.png から変わっているのは、Cloudflare Pages のビルド
      // キャッシュが古い画像を出し続けたため (2026-09-04)。中身を差し替えた
      // コミットで public/ の favicon は新しくなったのに、src/assets を通る
      // これだけが古いハッシュのまま公開された。名前を変えると確実に作り直る
      logo: { src: "./src/assets/app-icon.png", alt: "SHIKISHA-TERM" },
      favicon: "/favicon.ico",
      head: [
        // SNSに貼られたときのカード。og:image は絶対URLでないと拾われない
        { tag: "meta", attrs: { property: "og:image", content: "https://shikisha-term.com/og.png" } },
        { tag: "meta", attrs: { name: "twitter:card", content: "summary_large_image" } },
        { tag: "meta", attrs: { name: "twitter:image", content: "https://shikisha-term.com/og.png" } },
        { tag: "link", attrs: { rel: "apple-touch-icon", href: "/apple-touch-icon.png" } },
      ],
      social: [{ icon: "github", label: "GitHub", href: REPO }],
      // 各ページに「編集する」リンクが出る。翻訳PRの入り口になる
      editLink: { baseUrl: `${REPO}/edit/main/site/` },
      lastUpdated: true,
      sidebar: [
        { label: "Automation", link: "/automation/" },
        { label: "Translating", link: "/translating/" },
        { label: "Privacy", link: "/privacy/" },
        { label: "Microsoft Store", link: "https://apps.microsoft.com/detail/9PB8XQVM87Z0", attrs: { target: "_blank" } },
        { label: "Portable zip", link: `${REPO}/releases/latest`, attrs: { target: "_blank" } },
      ],
      customCss: ["./src/styles/custom.css"],
    }),
  ],
});

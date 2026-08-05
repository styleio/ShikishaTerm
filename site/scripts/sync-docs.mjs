// docs/*.md をサイトのページに変換する。
//
// 情報源は必ずリポジトリの docs/ 側ひとつだけにする。
// あれは exe に include_str! で埋め込まれ、AIに渡す仕様書でもあるので、
// サイト用にコピーを持つと必ず食い違う。ここで毎回作り直す。
import { mkdir, readFile, writeFile, rm } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repo = join(here, "..", "..");
const out = join(here, "..", "src", "content", "docs");

const REPO_URL = "https://github.com/styleio/ShikishaTerm";

/** src: リポジトリ上の位置 / dest: サイト上の位置 / order: サイドバーの並び */
const PAGES = [
  { src: "docs/AUTOMATION.md", dest: "automation.md", order: 1 },
  { src: "docs/TRANSLATING.md", dest: "translating.md", order: 2 },
  { src: "docs/AUTOMATION.ja.md", dest: "ja/automation.md", order: 1 },
];

/** 1行目の `# 見出し` をページタイトルに使い、本文からは取り除く */
function splitTitle(md) {
  const m = md.match(/^#\s+(.+?)\s*\n+/);
  return m ? { title: m[1], body: md.slice(m[0].length) } : { title: null, body: md };
}

/** 「最初の段落」を description に回す (検索結果とOGPに出る) */
function firstParagraph(body) {
  const p = body.split(/\n\s*\n/).find((s) => s.trim() && !s.startsWith("#") && !s.startsWith("|"));
  return p ? p.replace(/\s+/g, " ").replace(/[*`\[\]]/g, "").trim().slice(0, 160) : "";
}

/**
 * リポジトリ内への相対リンクはサイト上では壊れる。
 * サイトにも存在するページは相対リンクへ、それ以外は GitHub へ向ける。
 */
function fixLinks(body, dest) {
  const onSite = new Map([
    ["docs/AUTOMATION.md", "/automation/"],
    ["docs/AUTOMATION.ja.md", "/ja/automation/"],
    ["docs/TRANSLATING.md", "/translating/"],
  ]);
  return body.replace(/\]\((?!https?:|#|\/)([^)]+)\)/g, (whole, href) => {
    const clean = href.replace(/^\.\//, "");
    if (onSite.has(clean)) return `](${onSite.get(clean)})`;
    return `](${REPO_URL}/blob/main/${clean})`;
  });
}

function frontmatter({ title, description, order }) {
  const esc = (s) => `"${s.replace(/"/g, '\\"')}"`;
  return [
    "---",
    `title: ${esc(title)}`,
    description ? `description: ${esc(description)}` : null,
    "sidebar:",
    `  order: ${order}`,
    "---",
    "",
    "<!-- このファイルは docs/ から生成されています。編集は docs/ 側で行ってください -->",
    "",
  ]
    .filter(Boolean)
    .join("\n");
}

for (const page of PAGES) {
  const raw = await readFile(join(repo, page.src), "utf8");
  const { title, body } = splitTitle(raw);
  const target = join(out, page.dest);
  await mkdir(dirname(target), { recursive: true });
  await rm(target, { force: true });
  await writeFile(
    target,
    frontmatter({
      title: title ?? page.dest,
      description: firstParagraph(body),
      order: page.order,
    }) + fixLinks(body, page.dest),
    "utf8"
  );
  console.log(`${page.src} -> src/content/docs/${page.dest}`);
}

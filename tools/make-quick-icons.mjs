// Builds vendor/lucide/icons.tsv, the line icons a quick command can wear.
//
//   npm pack lucide-static@<version>   (anywhere), untar it, then
//   node tools/make-quick-icons.mjs <path to the untarred package>
//
// One line per icon: name, its search words, and the inside of its <svg>
// (everything between the tags; the outer tag is the same for all of them and
// is written by whoever draws one). Tab-separated because neither a name, a
// word nor SVG markup ever holds a tab, and a line a program can split without
// a parser is one both the Rust side and the page can read.
//
// The licence travels beside it as vendor/lucide/LICENSE and in
// THIRD-PARTY-NOTICES.txt.

import fs from "node:fs";
import path from "node:path";

const pkg = process.argv[2];
if (!pkg) {
  console.error("usage: node tools/make-quick-icons.mjs <lucide-static package folder>");
  process.exit(2);
}
const nodes = JSON.parse(fs.readFileSync(path.join(pkg, "icon-nodes.json"), "utf8"));
const tags = JSON.parse(fs.readFileSync(path.join(pkg, "tags.json"), "utf8"));
const version = JSON.parse(fs.readFileSync(path.join(pkg, "package.json"), "utf8")).version;

const esc = v => String(v).replace(/&/g, "&amp;").replace(/"/g, "&quot;").replace(/</g, "&lt;");
const lines = [];
for (const name of Object.keys(nodes).sort()) {
  if (!/^[a-z0-9-]+$/.test(name)) throw new Error("unexpected icon name: " + name);
  const inner = nodes[name]
    .map(([tag, attrs]) => {
      const a = Object.entries(attrs)
        .filter(([k]) => k !== "key")
        .map(([k, v]) => ` ${k}="${esc(v)}"`)
        .join("");
      return `<${tag}${a}/>`;
    })
    .join("");
  const words = (tags[name] || []).map(w => String(w).replace(/[\t\n,]/g, " ").trim()).filter(Boolean);
  for (const part of [name, inner, ...words]) {
    if (/[\t\n]/.test(part)) throw new Error("a tab or newline inside " + name);
  }
  lines.push([name, words.join(","), inner].join("\t"));
}
const out = path.join(path.dirname(new URL(import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, "$1")), "..", "vendor", "lucide");
fs.mkdirSync(out, { recursive: true });
fs.writeFileSync(path.join(out, "icons.tsv"), `# lucide-static ${version} (ISC)\n` + lines.join("\n") + "\n");
fs.copyFileSync(path.join(pkg, "LICENSE"), path.join(out, "LICENSE"));
console.log(`${lines.length} icons from lucide-static ${version}`);

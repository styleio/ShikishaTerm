// Take the download pages back out of the sitemap.
//
// /get/ and /ja/get/ start a file transfer the moment they are opened, so they
// carry <meta name="robots" content="noindex">. Starlight adds @astrojs/sitemap
// itself and gives us no filter, so a URL that says "do not index me" would
// still be submitted for indexing — which Search Console reports as an error.
// Cheaper to strike the two lines out afterwards than to fight the integration.
import { readFile, writeFile } from "node:fs/promises";
import { existsSync } from "node:fs";

const FILE = new URL("../dist/sitemap-0.xml", import.meta.url);
const NOINDEX = ["/get/", "/ja/get/"];

if (!existsSync(FILE)) {
  console.warn("prune-sitemap: dist/sitemap-0.xml is not there, nothing to do");
  process.exit(0);
}

const before = await readFile(FILE, "utf8");
const after = before.replace(/<url>[\s\S]*?<\/url>/g, (entry) => {
  const loc = entry.match(/<loc>([^<]+)<\/loc>/)?.[1] ?? "";
  return NOINDEX.some((path) => new URL(loc).pathname === path) ? "" : entry;
});

if (after === before) {
  console.warn(`prune-sitemap: none of ${NOINDEX.join(", ")} were in the sitemap`);
} else {
  await writeFile(FILE, after);
  console.log(`prune-sitemap: dropped ${NOINDEX.join(", ")}`);
}

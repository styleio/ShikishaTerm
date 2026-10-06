// Build the Markdown kit the board carries.
//
//   cd tools/mdkit && npm ci && node build.mjs
//
// Writes vendor/mdkit/: mdkit.js (the kit, window.MdKit), mermaid.js (the
// diagram library, fetched by the page only when a document has a diagram),
// katex.css and its fonts. The versions are pinned in package.json and
// package-lock.json; what is written is what crates/core/src/mdkit.rs carries.
import { build } from 'esbuild';
import fs from 'node:fs';
import path from 'node:path';

const here = import.meta.dirname;
const out = path.resolve(here, '..', '..', 'vendor', 'mdkit');
fs.rmSync(out, { recursive: true, force: true });
fs.mkdirSync(path.join(out, 'fonts'), { recursive: true });

const bundled = await build({
  metafile: true,
  entryPoints: [path.join(here, 'src', 'index.js')],
  bundle: true,
  minify: true,
  format: 'iife',
  globalName: 'MdKit',
  target: ['es2022'],
  outfile: path.join(out, 'mdkit.js'),
  legalComments: 'none',
  // One KaTeX for the reader and the editor alike, not one each
  alias: { katex: path.join(here, 'node_modules', 'katex') },
  logLevel: 'warning',
});

const nm = path.join(here, 'node_modules');
fs.copyFileSync(path.join(nm, 'mermaid', 'dist', 'mermaid.min.js'), path.join(out, 'mermaid.js'));
fs.copyFileSync(path.join(nm, 'katex', 'dist', 'katex.min.css'), path.join(out, 'katex.css'));
for (const f of fs.readdirSync(path.join(nm, 'katex', 'dist', 'fonts'))) {
  if (f.endsWith('.woff2')) fs.copyFileSync(path.join(nm, 'katex', 'dist', 'fonts', f), path.join(out, 'fonts', f));
}
// Only the woff2 fonts are carried: every browser this page runs in reads
// them, and the stylesheet's other formats are fallbacks it never asks for
const css = fs.readFileSync(path.join(out, 'katex.css'), 'utf8')
  .replace(/,\s*url\([^)]*\.woff\)\s*format\("woff"\)/g, '')
  .replace(/,\s*url\([^)]*\.ttf\)\s*format\("truetype"\)/g, '');
fs.writeFileSync(path.join(out, 'katex.css'), css);

// The notices of what was carried: every package a byte of the kit came from,
// with the diagram library and KaTeX (whose stylesheet and fonts go too),
// each with its licence as the package ships it. Read by tools/notices.ps1
const used = new Set(['mermaid', 'katex']);
for (const input of Object.keys(bundled.metafile.inputs)) {
  const m = /node_modules\/((?:@[^/]+\/)?[^/]+)\//.exec(input.replace(/\\/g, '/'));
  if (m) used.add(m[1]);
}
// The diagram library comes built, with its own dependencies inside it and
// no list of them: every package it depends on is noticed, which is at least
// what it carries
const walk = (name, seen) => {
  const file = path.join(nm, ...name.split('/'), 'package.json');
  if (seen.has(name) || !fs.existsSync(file)) return;
  seen.add(name);
  used.add(name);
  const pkg = JSON.parse(fs.readFileSync(file, 'utf8'));
  for (const dep of Object.keys(pkg.dependencies || {})) walk(dep, seen);
};
walk('mermaid', new Set());
const notices = [...used].sort().map(name => {
  const dir = path.join(nm, ...name.split('/'));
  const pkg = JSON.parse(fs.readFileSync(path.join(dir, 'package.json'), 'utf8'));
  const file = fs.readdirSync(dir).find(f => /^(licen[cs]e|copying)(\.|$)/i.test(f));
  const text = file ? fs.readFileSync(path.join(dir, file), 'utf8').replace(/\r/g, '').trim()
    : `${pkg.license} -- ${pkg.name} ${pkg.version} ships no licence file; see ${pkg.repository && (pkg.repository.url || pkg.repository) || pkg.homepage || 'its package'}`;
  // Where its source is: what a licence asking for the source to be findable
  // (EPL, MPL) is answered with, and useful for every other one
  const repo = pkg.repository && (typeof pkg.repository === 'string' ? pkg.repository : pkg.repository.url);
  const source = (repo || pkg.homepage || `https://www.npmjs.com/package/${name}`)
    .replace(/^git\+/, '').replace(/^git:\/\//, 'https://').replace(/\.git$/, '');
  return `--- ${name} ${pkg.version} (${pkg.license || 'see below'}) ---\nSource: ${source}\n\n${text}\n`;
});
fs.writeFileSync(path.join(out, 'NOTICES.txt'), notices.join('\n'));

const size = f => (fs.statSync(path.join(out, f)).size / 1024).toFixed(0) + ' KB';
console.log('mdkit.js', size('mdkit.js'), '· mermaid.js', size('mermaid.js'), '· katex.css', size('katex.css'),
  '·', fs.readdirSync(path.join(out, 'fonts')).length, 'fonts', '·', used.size, 'packages noticed');

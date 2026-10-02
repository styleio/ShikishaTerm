/**
 * Photograph the board page, in every language and scheme and at both widths.
 *
 * The screen rules (docs/design/STYLEGUIDE.md, section 9) ask every session
 * that changes the screen to look at a photograph of it and mark it against a
 * list. Photographing through the app means a build, a window, and whatever the
 * screen has to be showing at the time -- a server to be connected to, a
 * repository mid-merge, a transfer half done. None of that is the thing being
 * judged, and arranging it is how a session ends up judging the wrong screen.
 *
 * So the page is written out as it is served (src/bin/page_dump.rs), opened in
 * a headless Chrome, put into the state being judged by a line of its own
 * JavaScript, and photographed. What comes out is the real page: the real
 * stylesheet, the real wording from lang/, the real scheme from theme.rs.
 *
 *     node tools/debug/shoot.mjs tools/debug/scenes/files.mjs
 *     node tools/debug/shoot.mjs tools/debug/scenes/files.mjs --only diff
 *
 * Pictures land in target/shots. A scene file is described in the one there is.
 */
import {findChrome, findCargo, startChrome} from './chrome.mjs';
import fs from 'node:fs';
import path from 'node:path';
import { spawn, spawnSync } from 'node:child_process';
import { pathToFileURL } from 'node:url';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const OUT = path.join(ROOT, 'target', 'shots');

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const die = (why) => { console.error(why); process.exit(1); };

/**
 * The page as the app serves it, one file per language, scheme and side.
 *
 * "side" is who is being served: the window, or a phone. The two are not the
 * same page at different widths -- the phone's carries controls the window has
 * no use for -- so a scene about a phone's screen asks for `served: 'remote'`.
 */
function writePages(langs, looks, sides, page) {
  const cargo = findCargo();
  fs.mkdirSync(OUT, { recursive: true });
  const pages = {};
  for (const lang of langs) {
    for (const look of looks) {
      for (const side of sides) {
        const args = ['run', '--quiet', '--bin', 'page_dump', '--', lang];
        if (look === 'light') args.push('light');
        if (side === 'remote') args.push('remote');
        if (page) args.push(page);
        const made = spawnSync(cargo, args, { cwd: ROOT, maxBuffer: 1 << 28 });
        if (made.status !== 0) die('page_dump failed: ' + made.stderr);
        const file = path.join(OUT, [page || 'page', process.pid, lang, look, side, 'html'].join('.'));
        fs.writeFileSync(file, made.stdout);
        pages[lang + '.' + look + '.' + side] = file;
      }
    }
  }
  return pages;
}

/** A talking connection to one tab of a headless Chrome. */
const connect = chrome => startChrome({chrome});

const file = process.argv[2];
if (!file) die('say which scenes to photograph: node tools/debug/shoot.mjs <scenes.mjs>');
const only = process.argv.includes('--only')
  ? process.argv[process.argv.indexOf('--only') + 1]
  : null;
const spec = (await import(pathToFileURL(path.resolve(file)).href)).default;

const langs = spec.langs || ['en', 'ja'];
const looks = spec.looks || ['dark', 'light'];
const sizes = spec.sizes || [['wide', 1280, 860], ['phone', 390, 820]];
const served = spec.served || 'window';
// Only the sides some scene actually asks for: each one is another build
const sides = [...new Set(Object.values(spec.scenes || {})
  .map((s) => (typeof s === 'string' ? served : s.served || served)))];
const pages = writePages(langs, looks, sides, spec.page);
const chrome = await connect(findChrome());

// The page talks to the app through a bridge that is not here. A place that
// swallows what it is told, installed before the page's own script runs --
// that script gives up on its first throw, and everything below it with it
await chrome.send('Page.enable');
await chrome.send('Runtime.enable');
await chrome.send('Page.addScriptToEvaluateOnNewDocument',
  { source: 'window.ipc = { postMessage(){} };' });

let taken = 0;
try {
for (const [name, scene] of Object.entries(spec.scenes)) {
  if (only && name !== only) continue;
  const at = typeof scene === 'string' ? { run: scene } : scene;
  for (const lang of at.langs || langs) {
    for (const look of at.looks || looks) {
      for (const [size, w, h] of at.sizes || sizes) {
        await chrome.send('Emulation.setDeviceMetricsOverride',
          { width: w, height: h, deviceScaleFactor: 2, mobile: size === 'phone' });
        // A phone is a touch screen, not only a narrow one: what the page
        // draws only where there is no pointer to rest ("@media (hover: none)")
        // is drawn for it the way a real phone draws it
        await chrome.send('Emulation.setTouchEmulationEnabled',
          { enabled: size === 'phone', maxTouchPoints: size === 'phone' ? 5 : 1 });
        await chrome.send('Page.navigate',
          { url: pathToFileURL(pages[lang + '.' + look + '.' + (at.served || served)]).href });
        await sleep(spec.settle || 900);
        if (spec.setup) await chrome.run(spec.setup);
        await chrome.run(at.run);
        await sleep(250);
        const shot = await chrome.send('Page.captureScreenshot', { format: 'png' });
        const out = path.join(OUT, name + '-' + lang + '-' + look + '-' + size + '.png');
        fs.writeFileSync(out, Buffer.from(shot.data, 'base64'));
        console.log(path.relative(ROOT, out));
        taken += 1;
      }
    }
  }
}
} finally { chrome.stop(); }
if (!taken) die(only ? 'no scene called ' + only : 'the scene file has no scenes');

/**
 * Check password entry against the real loopback server in a fresh headless
 * browser. Delay state, authentication and reload responses beyond the poll
 * interval to reproduce overlapping prompts. Also check a remembered login,
 * repeated authentication and rejection of an incorrect first password.
 *
 * node tools/debug/remote-password.mjs
 * Needs Node, cargo and Chrome. Uses the ignored hold_a_reply_page_open test
 * with its own device book; no app, user settings or account is touched.
 * English/desktop and Japanese/phone screenshots go to target/shots/remote-password.
 */
import fs from 'node:fs';
import path from 'node:path';
import http from 'node:http';
import net from 'node:net';
import assert from 'node:assert/strict';
import {spawn, spawnSync} from 'node:child_process';
import {findCargo, startChrome} from './chrome.mjs';

const root = path.resolve(import.meta.dirname, '../..');
const shots = path.join(root, 'target/shots/remote-password');
const password = 'browser-test-password';
const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
async function until(check, what) {
  const end = Date.now() + 20000;
  while (Date.now() < end) { if (await check()) return; await pause(50); }
  throw new Error('Timed out: ' + what);
}

const built = spawnSync(findCargo(), ['test', '-p', 'shikisha-core', '--lib', '--no-run', '--message-format=json'],
  {cwd: root, encoding: 'utf8', maxBuffer: 32 * 1024 * 1024});
assert.equal(built.status, 0, built.stderr);
const exe = built.stdout.split(/\r?\n/).filter(line => line.startsWith('{')).map(line => JSON.parse(line))
  .find(row => row.reason === 'compiler-artifact' && row.target.name === 'shikisha_core' && row.executable)?.executable;
assert.ok(exe, 'the test executable was not reported by cargo');
fs.mkdirSync(shots, {recursive: true});

for (const [lang, width, height] of [['en', 1280, 860], ['ja', 390, 820]]) {
  const server = spawn(exe, ['--ignored', '--exact', 'remote::tests::hold_a_reply_page_open', '--nocapture'],
    {cwd: root, env: {...process.env, SHIKISHA_HOLD_PW: password, SHIKISHA_HOLD_LANG: lang},
      stdio: ['ignore', 'pipe', 'pipe'], windowsHide: true});
  let output = '', chrome, proxy;
  const sockets = new Set(), timers = new Set();
  server.stdout.on('data', b => output += b);
  server.stderr.on('data', b => output += b);
  try {
    await until(() => output.includes('REPLY LINK:'), 'the isolated server');
    const origin = new URL(output.match(/REPLY LINK: (http:\/\/\S+)/)[1]);
    let roots = 0, auths = 0, polls = 0;
    proxy = http.createServer((req, res) => {
      const url = new URL(req.url, origin);
      // Each delay exceeds, or straddles, the 1500 ms poll. Keep it short
      // enough that the check finishes before the fixture's three minutes.
      const delay = url.pathname === '/auth' ? (++auths === 1 ? 2200 : 0)
        : url.pathname === '/api/state' ? (++polls === 1 ? 2100 : 0)
        : url.pathname === '/' ? (++roots > 1 ? 1200 : 0) : 0;
      // Keep Host and Origin together: this is a transport delay, not an
      // attempt to loosen the server's origin checks.
      const upstream = http.request(url, {method: req.method, headers: req.headers}, response => {
        const chunks = [];
        response.on('data', b => chunks.push(b));
        response.on('end', () => {
          const timer = setTimeout(() => {
            timers.delete(timer);
            if (res.destroyed) return;
            res.writeHead(response.statusCode, response.headers);
            res.end(Buffer.concat(chunks));
          }, delay);
          timers.add(timer);
        });
      });
      upstream.on('error', () => { if (!res.destroyed) { res.writeHead(502); res.end(); } });
      req.pipe(upstream);
    });
    const track = socket => { sockets.add(socket); socket.on('close', () => sockets.delete(socket)); };
    proxy.on('connection', track);
    proxy.on('upgrade', (req, socket, head) => {
      const upstream = net.connect(Number(origin.port), '127.0.0.1', () => {
        upstream.write(`${req.method} ${req.url} HTTP/${req.httpVersion}\r\n`
          + Object.entries(req.headers).map(([k, v]) => `${k}: ${v}`).join('\r\n') + '\r\n\r\n');
        if (head.length) upstream.write(head);
        socket.pipe(upstream); upstream.pipe(socket);
      });
      track(upstream);
      upstream.on('error', () => socket.destroy());
      socket.on('error', () => upstream.destroy());
      socket.on('close', () => upstream.destroy());
    });
    await new Promise(resolve => proxy.listen(0, '127.0.0.1', resolve));
    const url = `http://127.0.0.1:${proxy.address().port}/?t=board-token-0000`;
    const T = JSON.parse(fs.readFileSync(path.join(root, 'lang', lang + '.json'), 'utf8'));
    chrome = await startChrome();
    for (const domain of ['Page', 'Runtime', 'Network']) await chrome.send(domain + '.enable');
    await chrome.send('Emulation.setDeviceMetricsOverride', {width, height, deviceScaleFactor: 1, mobile: width < 600});
    let prompts = 0, wrong = false, errors = [];
    const alerts = [];
    chrome.ws.addEventListener('message', event => {
      const message = JSON.parse(event.data);
      if (message.method !== 'Page.javascriptDialogOpening') return;
      const dialog = message.params;
      void (async () => {
        if (dialog.type === 'prompt') {
          prompts++;
          assert.equal(dialog.message, T['tui.remote.password_prompt']);
          // A person takes longer to type than the polling interval.
          await pause(wrong ? 100 : 2600);
          await chrome.send('Page.handleJavaScriptDialog', {accept: true, promptText: wrong ? 'incorrect' : password});
        } else {
          alerts.push(dialog.message);
          await chrome.send('Page.handleJavaScriptDialog', {accept: true});
        }
      })().catch(error => errors.push(error));
    });
    await chrome.send('Page.navigate', {url});
    await until(() => roots >= 2, 'successful authentication to reload the page');
    await pause(1800); // let the delayed reload and any obsolete poll settle
    assert.equal(prompts, 1, 'the password was asked more than once');
    assert.equal(auths, 1, 'more than one authentication request was sent');
    assert.deepEqual(alerts, [], 'a successful login displayed an error');
    assert.equal(await chrome.run(`fetch('/api/state').then(r => r.status)`), 200);
    for (const value of [password, 'incorrect']) {
      assert.equal(await chrome.run(`fetch('/auth', {method:'POST',headers:{'Content-Type':'application/json'},
        body:JSON.stringify({password:${JSON.stringify(value)}})}).then(r => r.status)`), 200);
    }
    const before = roots;
    await chrome.send('Page.reload');
    await until(() => roots > before, 'a remembered device to reload');
    await pause(1800);
    assert.equal(prompts, 1, 'a remembered device was asked again');
    const picture = await chrome.send('Page.captureScreenshot', {format: 'png'});
    fs.writeFileSync(path.join(shots, lang + '.png'), Buffer.from(picture.data, 'base64'));

    // The same browser with no credentials is once again a locked visitor.
    await chrome.send('Network.clearBrowserCookies');
    wrong = true;
    await chrome.send('Page.navigate', {url});
    await until(() => alerts.length > 0, 'a wrong password to be refused');
    assert.equal(alerts[0], T['tui.remote.password_wrong']);
    assert.equal(await chrome.run(`fetch('/api/state').then(r => r.status)`), 403);
    assert.deepEqual(errors, []);
    assert.deepEqual(chrome.thrown, []);
    console.log(`PASS ${lang}/${width}: one prompt under delay, remembered login, repeated auth, wrong password refused`);
  } finally {
    chrome?.stop();
    for (const timer of timers) clearTimeout(timer);
    for (const socket of sockets) socket.destroy();
    proxy?.close();
    server.kill();
  }
}

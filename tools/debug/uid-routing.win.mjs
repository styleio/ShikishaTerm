// Desk/tab identity regression through the app's real HTTP and pipe doors.
// Windows + Node, after `cargo build --bin SHIKISHA-TERM`.
// Run: node tools/debug/uid-routing.win.mjs
// Uses an isolated instance and synthetic terminals; no accounts or AI calls.
import fs from 'node:fs';
import path from 'node:path';
import http from 'node:http';
import net from 'node:net';
import crypto from 'node:crypto';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import assert from 'node:assert/strict';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
const target = path.join(root, 'target');
const run = path.join(target, 'uid-routing-' + crypto.randomUUID());
const instance = path.resolve(run, 'instance');
// instance.win.ps1 replaces its destination recursively. Verify the absolute
// destination before giving it to that script, including on failure cleanup.
assert(instance.startsWith(target + path.sep) && instance !== target);
const fixture = path.join(run, 'fixture');
fs.mkdirSync(fixture, { recursive: true });
const exe = path.join(target, 'debug', 'SHIKISHA-TERM.exe');
const hash = file => crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex');
const ps = (...args) => {
  const r = spawnSync('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-File',
    path.join(root, 'tools/debug/instance.win.ps1'), '-At', instance, ...args],
  { cwd: root, windowsHide: true, encoding: 'utf8', timeout: 90000 });
  if (r.status !== 0) throw new Error(r.stderr || r.stdout || String(r.error));
  return r.stdout;
};
const files = spawnSync('git', ['ls-files', 'crates', 'src', 'lang', 'Cargo.toml', 'Cargo.lock', 'build.rs'],
  { cwd: root, windowsHide: true, encoding: 'utf8' });
assert.equal(files.status, 0);
const built = fs.statSync(exe).mtimeMs;
assert(files.stdout.trim().split(/\r?\n/).every(f => fs.statSync(path.join(root, f)).mtimeMs <= built),
  'Build the latest sources before testing');
fs.writeFileSync(path.join(fixture, 'listen.cjs'), `
const fs = require('node:fs'), path = require('node:path');
const label = process.argv[2];
fs.writeFileSync(path.join(__dirname, label + '.auth.json'), JSON.stringify({pipe: process.env.SHIKISHA_PIPE, token: process.env.SHIKISHA_TOKEN}));
if (process.stdin.isTTY) process.stdin.setRawMode(true);
process.stdout.write('READY ' + label + '\\r\\n> ');
process.stdin.on('data', d => { fs.appendFileSync(path.join(__dirname, label + '.heard'), d); process.stdout.write(d); });
setInterval(() => {}, 1000);
`);
fs.writeFileSync(path.join(fixture, 'page.html'), '<!doctype html><title>Test page</title><p>Identity test</p>');
const token = crypto.randomBytes(24).toString('hex');
const desks = ['A', 'B'].map(name => {
  const cwd = path.join(fixture, name);
  fs.mkdirSync(cwd);
  return { id: name.toLowerCase(), uid: crypto.randomUUID(), name,
    projects: [{ name: 'app', uid: crypto.randomUUID(), homes: [{host: 'test-ssh', at: '/repo/' + name.toLowerCase()}] }],
    folders: [{ cwd, tabs: ['caller', 'worker'].map(id => ({id, name: id + '-' + name, uid: crypto.randomUUID(),
      command: [process.execPath, path.join(fixture, 'listen.cjs'), id + '-' + name]})) },
    {cwd: '/repo/' + name.toLowerCase(), host: 'test-ssh', project: 'app', tabs: []}] };
});
const cfg = {language: 'en', desks, keep_terminals: false,
  agent_hooks: {'Codex CLI': 'off', 'Claude Code': 'off', 'Gemini CLI': 'off'},
  hosts: [{name: 'test-ssh', at: 'ssh://test@127.0.0.1:1'}], remote: {fixed_token: token, sticky_token: true}};
const config = path.join(fixture, 'config.json');
fs.writeFileSync(config, JSON.stringify(cfg));
let port, cookie = '', checks = [];
const sleep = ms => new Promise(r => setTimeout(r, ms));
const until = async (test, what, ms = 20000) => {
  const end = Date.now() + ms;
  while (Date.now() < end) { if (await test()) return; await sleep(200); }
  throw new Error('Timed out: ' + what);
};
const check = (value, what) => { assert(value, what); checks.push(what); console.log('PASS ' + what); };
const request = (url, body) => new Promise((resolve, reject) => {
  const payload = body === undefined ? undefined : JSON.stringify(body);
  const req = http.request({host: '127.0.0.1', port, path: url, agent: false, method: payload ? 'POST' : 'GET',
    headers: {Cookie: cookie, Origin: 'http://127.0.0.1:' + port,
      ...(payload ? {'Content-Type': 'application/json', 'Content-Length': Buffer.byteLength(payload)} : {})}}, res => {
    let text = '';
    res.setEncoding('utf8'); res.on('data', part => text += part);
    res.on('end', () => {
      const set = res.headers['set-cookie'] || [];
      cookie = [cookie, ...set.map(c => c.split(';')[0])].filter(Boolean).join('; ');
      let data; try { data = JSON.parse(text); } catch { data = null; }
      resolve({status: res.statusCode, data});
    });
  });
  req.setTimeout(10000, () => req.destroy(new Error('HTTP request timed out')));
  req.on('error', reject); req.end(payload);
});
const auth = label => JSON.parse(fs.readFileSync(path.join(fixture, label + '.auth.json'), 'utf8'));
const call = (label, method, params = []) => new Promise((resolve, reject) => {
  const identity = auth(label), id = crypto.randomUUID();
  const socket = net.connect(identity.pipe);
  let buffer = '', entered = false;
  const timer = setTimeout(() => { socket.destroy(); reject(new Error('Pipe timed out: ' + method)); }, 12000);
  socket.on('error', e => {clearTimeout(timer); reject(e);});
  socket.on('connect', () => socket.write(JSON.stringify({token: identity.token}) + '\n'));
  socket.on('data', d => {
    buffer += d.toString();
    for (;;) {
      const end = buffer.indexOf('\n'); if (end < 0) break;
      const response = JSON.parse(buffer.slice(0, end)); buffer = buffer.slice(end + 1);
      if (!entered) {
        if (response.ok === false) {clearTimeout(timer); socket.destroy(); reject(new Error('Handshake refused')); return;}
        entered = true; socket.write(JSON.stringify({id, method, params}) + '\n');
      } else if (response.id === id) {clearTimeout(timer); socket.destroy(); resolve(response);}
    }
  });
});
const heard = label => {const f = path.join(fixture, label + '.heard'); return fs.existsSync(f) ? fs.readFileSync(f, 'utf8') : '';};
const state = async () => (await request('/api/state')).data;
const intent = body => request('/api/intent', body);
const list = async label => {const r = await call(label, 'tab_list'); assert(r.ok, r.error); return r.result;};
const liveConfig = path.join(instance, 'app/config/config.json');
const edit = change => {const c = JSON.parse(fs.readFileSync(liveConfig, 'utf8')); change(c); fs.writeFileSync(liveConfig, JSON.stringify(c));};

try {
  const started = ps('-Config', config);
  port = Number(started.match(/board=http:\/\/127\.0\.0\.1:(\d+)/)?.[1]);
  assert(port > 0);
  check(hash(exe) === hash(path.join(instance, 'app/SHIKISHA-TERM.exe')), 'the tested executable matches the latest build');
  await request('/?t=' + token);
  await request('/cfg?t=' + token);
  await until(async () => fs.existsSync(path.join(fixture, 'caller-A.auth.json')), 'desk A terminal');
  check((await list('caller-A')).find(t => t.you)?.name === 'caller-A', 'the caller is identified on desk A');
  await intent({kind: 'runkey', name: 'desk_next'});
  await until(async () => (await state()).desk === 'B' && fs.existsSync(path.join(fixture, 'caller-B.auth.json')), 'desk B');
  await sleep(2500);
  check((await list('caller-A')).find(t => t.you)?.name === 'caller-A', 'switching desks keeps the API caller on desk A');
  const apiMark = 'API-' + crypto.randomUUID();
  assert((await call('caller-A', 'send_to_tab', ['worker', apiMark])).ok);
  await until(async () => heard('worker-A').includes(apiMark), 'API delivery to background A');
  check(!heard('worker-B').includes(apiMark), 'a background API send reaches only its own worker');
  const uidA = desks[0].folders[0].tabs[1].uid;
  const stale = await request('/api/send', {tab: 2, text: 'MISSING-UID'});
  check(stale.status === 409 && stale.data.ok === false, 'a legacy message without a UID is refused');
  const composerMark = 'COMPOSER-' + crypto.randomUUID();
  assert((await intent({kind: 'say', tab: 2, uid: uidA, text: composerMark})).data.ok);
  await until(async () => heard('worker-A').includes(composerMark), 'stale view delivery to A');
  check(!heard('worker-B').includes(composerMark), 'a message from an old view keeps its original recipient');
  for (const d of desks) {
    const r = await request('/api/folder/rename', {path: '\u0001test-ssh\u0001/repo/' + d.id,
      desk: d.uid, name: 'renamed', from: 'main', go: false});
    check(r.data?.ok === false && /own folder/.test(r.data.error), 'same-name project ' + d.name + ' keeps its own main checkout');
  }
  const page = {id: 'test-page', uid: crypto.randomUUID(), name: 'Before', command: ['browser', path.join(fixture, 'page.html')]};
  edit(c => c.desks[1].folders[0].tabs.push(page));
  await until(async () => (await list('caller-B')).some(t => t.id === page.id), 'first page');
  await intent({kind: 'say', tab: 1, uid: desks[1].folders[0].tabs[0].uid, text: 'Inspect <@test-page>'});
  await until(async () => (await list('caller-B')).some(t => t.id === page.id && t.named), 'permission for first page');
  check(true, 'a new mention grants the existing page');
  edit(c => c.desks[1].folders[0].tabs = c.desks[1].folders[0].tabs.filter(t => t.id !== page.id));
  await until(async () => !(await list('caller-B')).some(t => t.id === page.id), 'first page closed');
  edit(c => c.desks[1].folders[0].tabs.push({...page, uid: crypto.randomUUID(), name: 'After'}));
  await until(async () => (await list('caller-B')).some(t => t.id === page.id && t.name === 'After'), 'replacement page');
  check(!(await list('caller-B')).find(t => t.id === page.id).named, 'a replacement page cannot inherit an old mention');
  await intent({kind: 'say', tab: 1, uid: desks[1].folders[0].tabs[0].uid, text: 'Inspect <@test-page> again'});
  await until(async () => (await list('caller-B')).some(t => t.id === page.id && t.named), 'permission for replacement page');
  check(true, 'a fresh mention can grant the replacement page');
  edit(c => {c.desks[0].name = 'A restricted'; c.desks[0].automation_permissions = {send_to_tab: {human: false, ai: false}};});
  await until(async () => JSON.stringify((await state()).ui?.desks).includes('A restricted'), 'reloaded permission settings');
  const denied = await call('caller-A', 'send_to_tab', ['worker', 'MUST-NOT-SEND']);
  check(!denied.ok, 'a background call observes its own desk permission');
  const ownMark = 'OWN-' + crypto.randomUUID();
  assert((await call('caller-B', 'send_to_tab', ['worker', ownMark])).ok);
  await until(async () => heard('worker-B').includes(ownMark), 'foreground delivery after a refused background call');
  check(!heard('worker-A').includes(ownMark), 'the foreground permission and recipient are restored');
  // Replace a terminal on the viewed desk; an old request must not go to its
  // namesake even though that new tab occupies the very same row.
  const oldUid = desks[1].folders[0].tabs[1].uid;
  edit(c => c.desks[1].folders[0].tabs.splice(1, 1));
  await until(async () => !(await list('caller-B')).some(t => t.id === 'worker'), 'worker closed');
  edit(c => c.desks[1].folders[0].tabs.splice(1, 0, {...desks[1].folders[0].tabs[1], uid: crypto.randomUUID(), name: 'replacement',
    command: [process.execPath, path.join(fixture, 'listen.cjs'), 'replacement']}));
  await until(async () => fs.existsSync(path.join(fixture, 'replacement.auth.json')), 'replacement worker');
  const goneMark = 'GONE-' + crypto.randomUUID();
  await intent({kind: 'say', tab: 2, uid: oldUid, text: goneMark});
  await sleep(1200);
  check(!heard('replacement').includes(goneMark), 'a deleted recipient never forwards to its replacement');
  fs.writeFileSync(path.join(run, 'result.json'), JSON.stringify({testedAt: new Date().toISOString(), exeSha256: hash(exe), checks}, null, 2));
  console.log('Evidence: ' + path.join(run, 'result.json'));
} finally {
  ps('-Stop');
}

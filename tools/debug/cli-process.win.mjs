// CLI process identity regression through the app's real HTTP and pipe doors.
// Windows + Node, after `cargo build --bin SHIKISHA-TERM`.
// Run: node tools/debug/cli-process.win.mjs
// --real starts two installed Codex CLIs over an empty CODEX_HOME, without
// sending a prompt. Otherwise uses synthetic terminals. No AI calls.
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
const run = path.join(target, 'cli-process-' + crypto.randomUUID());
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
const files = spawnSync('git', ['ls-files', 'crates', 'src', 'lang', 'Cargo.toml', 'Cargo.lock', 'build.rs', 'profiles'],
  { cwd: root, windowsHide: true, encoding: 'utf8' });
assert.equal(files.status, 0);
const built = fs.statSync(exe).mtimeMs;
assert(files.stdout.trim().split(/\r?\n/).every(f => fs.statSync(path.join(root, f)).mtimeMs <= built),
  'Build the latest sources before testing');
const token = crypto.randomBytes(24).toString('hex');
const checks = [];
let port, cookie = '';
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

// A CLI with a shared server: without the isolation option every tab's hook
// inherits the first tab's key. With it, each hook retains its own identity.
fs.writeFileSync(path.join(fixture, 'cli.cjs'), `
const fs = require('node:fs'), path = require('node:path'), net = require('node:net');
const mode = process.argv[2], args = process.argv.slice(3);
if (args.includes('--help')) { console.log(mode === 'old' ? 'Options: --search' : 'Options: --search --no-daemon'); process.exit(); }
if (mode === 'old' && args.includes('--no-daemon')) process.exit(2);
const label = process.env.SHIKISHA_TAB;
let identity = {pipe:process.env.SHIKISHA_PIPE, token:process.env.SHIKISHA_TOKEN};
const shared = path.join(__dirname, 'shared.json');
if (mode !== 'old' && !args.includes('--no-daemon')) {
  if (fs.existsSync(shared)) identity = JSON.parse(fs.readFileSync(shared));
  else fs.writeFileSync(shared, JSON.stringify(identity));
}
fs.writeFileSync(path.join(__dirname, label + '.auth.json'), JSON.stringify(identity));
fs.writeFileSync(path.join(__dirname, label + '.argv.json'), JSON.stringify(args));
async function call(method, params) {
  return new Promise((resolve, reject) => {
    const socket=net.connect(identity.pipe); let buffer='', ready=false;
    const timer=setTimeout(()=>{socket.destroy();reject(Error('timeout'))},8000);
    socket.on('connect',()=>socket.write(JSON.stringify({token:identity.token})+'\\n'));
    socket.on('error',reject);
    socket.on('data',d=>{buffer+=d;let i;while((i=buffer.indexOf('\\n'))>=0){
      const x=JSON.parse(buffer.slice(0,i));buffer=buffer.slice(i+1);
      if(!ready){ready=true;socket.write(JSON.stringify({id:'hook',method,params})+'\\n')}
      else{clearTimeout(timer);socket.destroy();resolve(x)}
    }});
  });
}
call('set_session', ['session-' + label]).then(()=>{
  process.stdout.write('READY '+label+'\\r\\n> ');
  fs.writeFileSync(path.join(__dirname,label+'.ready'),'ready');
});
if (process.stdin.isTTY) process.stdin.setRawMode(true);
process.stdin.on('data',()=>{});
setInterval(()=>{},1000);
`);
const folders = ['modern', 'old'].map(mode => {
  const cwd = path.join(fixture, mode + ' folder'); fs.mkdirSync(cwd);
  const command = path.join(cwd, 'codex.cmd');
  fs.writeFileSync(command, `@echo off\r\n"${process.execPath}" "${path.join(fixture, 'cli.cjs')}" ${mode} %*\r\n`);
  return {cwd, tabs: (mode === 'modern' ? ['alpha', 'beta', 'resume'] : ['legacy']).map(id => ({id,
    name: 'Same name', uid: crypto.randomUUID(), profile: 'Codex CLI',
    command: [command, ...(id === 'resume' ? ['resume', 'kept-conversation'] : [])]}))};
});
const cfg = {language: 'en', keep_terminals: process.argv.includes('--keeper'),
  agent_hooks: {'Codex CLI': 'off', 'Claude Code': 'off', 'Gemini CLI': 'off'},
  remote: {fixed_token: token, sticky_token: true},
  desks: [{id:'test', name:'Process identity', folders}]};
const real = process.argv.includes('--real');
if (real) {
  assert(!cfg.keep_terminals, '--real uses terminals owned by the test process');
  const located = spawnSync('where.exe', ['codex.cmd'], {windowsHide:true, encoding:'utf8'});
  assert.equal(located.status, 0, 'Codex CLI must be installed');
  const codex = located.stdout.trim().split(/\r?\n/)[0];
  const home = path.join(fixture, 'codex-home'); fs.mkdirSync(home);
  const command = path.join(fixture, 'real-codex.cmd');
  fs.writeFileSync(command, `@echo off\r\nset "CODEX_HOME=${home}"\r\ncall "${codex}" %*\r\n`);
  folders.splice(0, folders.length, {cwd:fixture, tabs:['alpha','beta'].map(id => ({id,name:'Same name',uid:crypto.randomUUID(),
    profile:'Codex CLI', command:[command]}))});
}
const config = path.join(fixture, 'config.json'); fs.writeFileSync(config, JSON.stringify(cfg));
const uid = id => folders.flatMap(f => f.tabs).find(t => t.id === id).uid;
try {
  const started = ps('-Config', config);
  port = Number(started.match(/board=http:\/\/127\.0\.0\.1:(\d+)/)?.[1]); assert(port > 0);
  check(hash(exe) === hash(path.join(instance, 'app/SHIKISHA-TERM.exe')), 'the tested executable matches the latest build');
  await request('/?t=' + token); await request('/cfg?t=' + token);
  if (real) {
    const pid = Number(started.match(/\bpid=(\d+)/)?.[1]); assert(pid > 0);
    let processes;
    await until(async () => {
      const read = spawnSync('powershell.exe', ['-NoProfile', '-Command',
        `$all=Get-CimInstance Win32_Process; $owned=@(${pid}); for($i=0;$i -lt 8;$i++) { $owned += @($all | Where-Object {$_.ParentProcessId -in $owned} | Select-Object -ExpandProperty ProcessId) }; $all | Where-Object {$_.ProcessId -in $owned} | Select-Object Name,ProcessId,ParentProcessId,CommandLine | ConvertTo-Json -Compress`],
        {windowsHide:true, encoding:'utf8', timeout:10000});
      assert.equal(read.status, 0);
      processes = JSON.parse(read.stdout);
      return processes.filter(p => p.Name === 'codex.exe' && p.CommandLine.includes('--no-daemon')).length === 2;
    }, 'two real Codex processes start independently', 40000);
    check(true, 'both installed Codex CLIs actually run with --no-daemon');
    check(!processes.some(p => p.CommandLine?.includes('--managed-daemon')), 'neither tab starts a shared daemon');
    const tabs = (await state()).tabs;
    check(tabs.length === 2 && tabs.every(t => t.state !== 'EXIT'), 'both real CLI terminals remain open');
    check(!fs.existsSync(path.join(fixture,'codex-home/app-server-daemon/daemon.pid')), 'the isolated Codex home has no shared daemon PID');
  } else {
  for (const id of ['alpha', 'beta', 'resume', 'legacy']) {
    await until(async () => fs.existsSync(path.join(fixture, id + '.ready')), id + ' started');
    check((await list(id)).find(t => t.you)?.id === id, id + ' retains its own API identity');
    const args = JSON.parse(fs.readFileSync(path.join(fixture, id + '.argv.json')));
    check(args.filter(a => a === '--no-daemon').length === (id === 'legacy' ? 0 : 1), id + ' gets only an option its CLI supports');
    if (id === 'resume') check(args.includes('kept-conversation') && args.includes('resume'), 'the explicit conversation is preserved');
  }
  await until(async () => (await state()).tabs.every(t => !['BUSY', 'BACKGROUND'].includes(t.state)), 'all tabs are idle');
  for (const id of ['alpha', 'beta']) {
    assert((await call(id, 'set_state', ['BUSY', Date.now()])).ok);
    await until(async () => (await state()).tabs.find(t => t.uid === uid(id))?.state === 'BUSY', id + ' busy');
    const s = await state();
    check(s.tabs.filter(t => t.uid !== uid(id)).every(t => !['BUSY','BACKGROUND'].includes(t.state)), id + ' working does not turn another tab green');
    assert((await call(id, 'set_state', ['DONE', Date.now()])).ok);
    await until(async () => (await state()).tabs.find(t => t.uid === uid(id))?.state !== 'BUSY', id + ' done');
    check(true, id + ' finishes independently');
  }
  await until(async () => {
    const file = path.join(instance, 'app/data/last-session');
    if (!fs.existsSync(file)) return false;
    const saved = JSON.parse(fs.readFileSync(file)).desks.flatMap(d => d.tabs);
    return ['alpha','beta','resume','legacy'].every(id => saved.some(t => t.id === id && t.session === 'session-' + id));
  }, 'each tab keeps its own conversation');
  check(true, 'conversation IDs are saved against their own tabs');
  }
  fs.writeFileSync(path.join(run, 'result.json'), JSON.stringify({testedAt:new Date().toISOString(), keeper:cfg.keep_terminals, real, exeSha256:hash(exe), checks},null,2));
  console.log('Evidence: ' + path.join(run, 'result.json'));
} finally { ps('-Stop'); }

// Open real terminal tabs from quick commands, through an isolated app.
// The commands leave markers in this run's folder; none restarts the app.
// Run after cargo build: node tools/debug/quick-commands.win.mjs
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import assert from 'node:assert/strict';
import {spawn} from 'node:child_process';

const root = path.resolve(import.meta.dirname, '../..');
const run = fs.mkdtempSync(path.join(os.tmpdir(), 'sk-quick-commands-'));
const instance = path.join(run, 'copy');
const work = path.join(run, 'work');
fs.mkdirSync(work);
const marker = path.join(work, 'quick-result.txt');
const script = path.join(work, 'command with spaces.cmd');
fs.writeFileSync(script, '@echo off\r\necho script>>quick-result.txt\r\n');
const config = path.join(run, 'config.json');
fs.writeFileSync(config, JSON.stringify({resident:false, quick_commands:{items:[
  {id:'probe', label:'Probe', body:"'shell' | Add-Content './quick-result.txt'", row:0, col:0},
  {id:'script', label:'Script', body:`& '${script.replaceAll("'", "''")}'`, row:0, col:1},
  {id:'draft', label:'Draft', body:"'draft' | Add-Content './quick-result.txt'", enter:false, row:0, col:2},
]}}));

const processOutput = (exe, args, input) => new Promise((resolve, reject) => {
  const child = spawn(exe, args, {windowsHide:true});
  let output = '';
  child.stdout.on('data', d => output += d);
  child.stderr.on('data', d => output += d);
  child.on('error', reject);
  child.on('exit', code => code === 0 ? resolve(output) : reject(new Error(output)));
  child.stdin.end(input);
});
const launcher = path.join(root, 'tools/debug/instance.win.ps1');
const ps = args => processOutput('powershell.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', launcher, ...args]);
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
const lines = () => fs.existsSync(marker) ? fs.readFileSync(marker, 'utf8').trim().split(/\r?\n/) : [];
const until = async (test, what) => {
  const end = Date.now() + 30000;
  while (Date.now() < end) {
    if (await test()) return;
    await sleep(150);
  }
  throw new Error('Timed out: ' + what);
};

try {
  console.log('Starting an isolated app; ' + run);
  const started = await ps(['-At', instance, '-Work', work, '-Config', config, '-Cdp', '0']);
  const base = /board=(http:\/\/127\.0\.0\.1:\d+)/.exec(started)?.[1];
  const pid = /\npid=(\d+)/.exec(started)?.[1];
  assert.ok(base && pid, 'the launcher named this copy');
  const mcp = async (name, params = []) => {
    const output = await processOutput(path.join(instance, 'app/SHIKISHA-TERM.exe'),
      ['--mcp', '--pid', pid, '--token-file', path.join(instance, 'app/data/api-token')],
      JSON.stringify({jsonrpc:'2.0', id:1, method:'tools/call', params:{name:'shikisha_' + name, arguments:{params}}}) + '\n');
    const reply = JSON.parse(output.trim()).result;
    assert.equal(reply.isError, false, output);
    return reply.content.map(c => c.text || '').join('');
  };
  const token = fs.readFileSync(path.join(instance, 'app/data/remote-token'), 'utf8').trim();
  const paired = await fetch(base + '/?t=' + encodeURIComponent(token), {redirect:'manual'});
  await paired.text();
  const cookie = paired.headers.getSetCookie().map(c => c.split(';')[0]).join('; ');
  assert.ok(cookie, 'the test paired with its own app');
  const press = async id => {
    const response = await fetch(base + '/api/intent', {method:'POST', headers:{Cookie:cookie, 'content-type':'application/json'},
      body:JSON.stringify({kind:'quick', id, tab:1})});
    assert.equal(response.status, 200);
    assert.equal((await response.json()).ok, true);
  };
  await until(async () => JSON.parse(await mcp('tab_list')).some(t => t.id === 'shell'), 'the starting tab');
  for (const [id, count] of [['probe', 1], ['probe', 2], ['script', 3]]) {
    await press(id);
    await until(() => lines().length >= count, id + ' executed');
    console.log('PASS command ' + count + ' reached its new tab');
  }
  assert.deepEqual(lines(), ['shell', 'shell', 'script']);
  await press('draft');
  await until(async () => {
    if (!JSON.parse(await mcp('tab_list')).some(t => t.id === 'draft')) return false;
    return (await mcp('tab_screen', ['draft'])).includes("'draft' | Add-Content");
  }, 'the draft was pasted');
  await sleep(1500);
  assert.deepEqual(lines(), ['shell', 'shell', 'script'], 'each submitted command ran once and the draft did not run');
  console.log('PASS disabling Enter pastes without running');
} finally {
  await ps(['-At', instance, '-Stop']);
}

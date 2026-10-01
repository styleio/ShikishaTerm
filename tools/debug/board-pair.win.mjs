/**
 * Pairing this PC with a server version's board (far-keep plan §6.2), with
 * the real server program, run in WSL on this PC:
 *
 *   1. `shikisha-server pair` prints a code (and the board's address)
 *   2. the code, given back once, is a key of this PC's own; given twice, it
 *      is refused
 *   3. the key gets a ticket, and the ticket -- once -- opens the board with
 *      the device's own key and a session, no token in the link
 *   4. the key asks how the board stands; a key nobody holds is refused
 *   5. a paired PC gets a code for a phone; the phone's link asks first and
 *      uses nothing, and its yes lets it in
 *   6. this PC's own side of it (the Rust functions the settings call) does
 *      the same against the same board (`cargo test ... live_board`)
 *   7. the board's key in a link writes no new device into the book, and the
 *      address the server prints carries no key
 *   8. wrong codes past the limit stop every code for a while
 *
 *     node tools/debug/board-pair.win.mjs
 *
 * Needs WSL with cargo. Builds shikisha-server there, runs it with a folder of
 * its own and the board on 127.0.0.1:8799, and stops it on the way out.
 */
import { spawnSync, spawn } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const WSL_ROOT = '/mnt/' + ROOT[0].toLowerCase() + ROOT.slice(2).replace(/\\/g, '/');
const PORT = 8799;
const BOARD = `http://127.0.0.1:${PORT}`;
const HOME = '/tmp/sk-board-pair';
const wsl = (script, opts = {}) => spawnSync('wsl', ['-e', 'bash', '-c', script], { encoding: 'utf8', env: { ...process.env, MSYS_NO_PATHCONV: '1' }, ...opts });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
let failures = 0;
const check = (ok, what) => { console.log((ok ? '  PASS ' : '  FAIL ') + what); if (!ok) failures += 1; };

console.log('building shikisha-server in WSL');
const built = wsl(`cd ${WSL_ROOT} && CARGO_TARGET_DIR=$HOME/sk-target ~/.cargo/bin/cargo build -q -p shikisha-server 2>&1 | tail -3`);
if (built.status !== 0) { console.error(built.stdout + built.stderr); process.exit(2); }
const SERVER = '$HOME/sk-target/debug/shikisha-server';
const config = JSON.stringify({
  language: 'en',
  remote: { enabled: true, bind: '0.0.0.0', port: PORT, allow_public: true },
  desks: [{ name: 'Srv', id: 'srv', folders: [{ cwd: '/tmp', tabs: [{ name: 'sh', id: 'sh', command: 'bash' }] }] }],
});
// Written here and copied there: a JSON text through two shells is a JSON text no more
const staged = path.join(process.env.TEMP, 'sk-board-pair-config.json');
fs.writeFileSync(staged, config);
const stagedWsl = '/mnt/' + staged[0].toLowerCase() + staged.slice(2).replace(/\\/g, '/');
wsl(`pkill -f [s]k-target/debug/shikisha-server ; rm -rf ${HOME} ; mkdir -p ${HOME}/config && cp ${stagedWsl} ${HOME}/config/config.json`);
const server = spawn('wsl', ['-e', 'bash', '-c', `SHIKISHA_HOME=${HOME} ${SERVER} > ${HOME}/out.log 2>&1`], { stdio: 'ignore', env: { ...process.env, MSYS_NO_PATHCONV: '1' } });
const post = async (p, body, key) => {
  const r = await fetch(BOARD + p, { method: 'POST', headers: { 'Content-Type': 'application/json', ...(key ? { 'X-Device-Key': key } : {}) }, body: JSON.stringify(body || {}) });
  return { status: r.status, json: await r.json().catch(() => null) };
};
try {
  let up = false;
  for (let i = 0; i < 60 && !up; i++) {
    await sleep(1000);
    up = await fetch(BOARD + '/pair/status', { signal: AbortSignal.timeout(3000) }).then((r) => r.status === 403).catch(() => false);
  }
  if (!up) throw new Error('the board did not come up: ' + wsl(`tail -5 ${HOME}/out.log`).stdout);

  console.log('1. shikisha-server pair');
  // The board writes its address down once it is serving; read by pair
  for (let i = 0; i < 30 && !wsl(`test -s ${HOME}/data/board-address && echo yes`).stdout.includes('yes'); i++) await sleep(1000);
  const said = wsl(`SHIKISHA_HOME=${HOME} ${SERVER} pair`).stdout;
  const code = (said.match(/[A-Z0-9]{4}-[A-Z0-9]{4}/) || [])[0];
  check(!!code, 'it prints a code: ' + said.split('\n')[0]);
  check(said.includes(`:${PORT}`), 'it prints the board\'s address');

  console.log('2. the code is a key, once');
  const paired = await post('/pair', { code, name: 'test PC' });
  const key = paired.json && paired.json.key;
  check(paired.json && paired.json.ok && typeof key === 'string' && key.length >= 32, 'the code is handed a key: ' + JSON.stringify(paired.json && { ...paired.json, key: key ? '…' : key }));
  const again = await post('/pair', { code, name: 'test PC' });
  check(again.json && !again.json.ok && again.json.why === 'unknown', 'the same code twice is refused: ' + JSON.stringify(again.json));

  console.log('3. a ticket opens the board, once, with the device\'s own key');
  const t = await post('/pair/ticket', {}, key);
  const ticket = t.json && t.json.ticket;
  check(!!ticket, 'the key gets a ticket');
  const page = await fetch(`${BOARD}/?ticket=${ticket}`, { redirect: 'manual' });
  const cookies = page.headers.getSetCookie ? page.headers.getSetCookie() : [];
  check(cookies.some((c) => c.startsWith('rk=' + key)) && cookies.some((c) => c.startsWith('rs=')), 'the ticket sets the device key and a session');
  const twice = await fetch(`${BOARD}/?ticket=${ticket}`, { redirect: 'manual' });
  const twiceCookies = twice.headers.getSetCookie ? twice.headers.getSetCookie() : [];
  check(!twiceCookies.some((c) => c.startsWith('rk=')), 'the same ticket twice lets nobody in');

  console.log('4. how the board stands, for a key it knows');
  const status = await fetch(BOARD + '/pair/status', { headers: { 'X-Device-Key': key } }).then((r) => r.json());
  check(status.ok && typeof status.tabs === 'number' && typeof status.working === 'number', 'the key is told how the board stands: ' + JSON.stringify(status));
  const stranger = await fetch(BOARD + '/pair/status', { headers: { 'X-Device-Key': 'not-a-key' } });
  check(stranger.status === 403, 'a key nobody holds is refused');

  console.log('5. a code for a phone');
  const inv = await post('/pair/invite', {}, key);
  const phoneCode = inv.json && inv.json.code;
  check(!!phoneCode, 'a paired PC gets a code for a phone');
  const plain = phoneCode.replace('-', '');
  const iphone = { 'User-Agent': 'Mozilla/5.0 (iPhone)' };
  // Opened twice, as a chat's preview and then the person would: neither uses the code
  for (const who of ['a preview', 'the person']) {
    const look = await fetch(`${BOARD}/?pair=${plain}`, { redirect: 'manual', headers: iphone });
    const lookCookies = look.headers.getSetCookie ? look.headers.getSetCookie() : [];
    const html = await look.text();
    check(look.status === 200 && !lookCookies.some((c) => c.startsWith('rk=')) && html.includes('action="/pair/arrive"'), `the link opened by ${who} asks, and lets nobody in yet`);
  }
  const yes = await fetch(BOARD + '/pair/arrive', { method: 'POST', redirect: 'manual', headers: { ...iphone, 'Content-Type': 'application/x-www-form-urlencoded' }, body: 'code=' + plain });
  const phoneCookies = yes.headers.getSetCookie ? yes.headers.getSetCookie() : [];
  check(yes.status === 303 && phoneCookies.some((c) => c.startsWith('rk=')) && phoneCookies.some((c) => c.startsWith('rs=')), 'its yes lets it in with a key of its own: ' + yes.status);
  const yesAgain = await fetch(BOARD + '/pair/arrive', { method: 'POST', redirect: 'manual', headers: { 'Content-Type': 'application/x-www-form-urlencoded' }, body: 'code=' + plain });
  check(yesAgain.status === 403, 'the same code said yes to twice is refused: ' + yesAgain.status);

  console.log('6. this PC\'s side, against the same board');
  // The Rust the settings call: paired by a code of its own, a ticket, how
  // the board stands, a code for a phone
  const pcCode = wsl(`SHIKISHA_HOME=${HOME} ${SERVER} pair`).stdout.match(/[A-Z0-9]{4}-[A-Z0-9]{4}/)[0];
  const cargo = spawnSync(path.join(process.env.USERPROFILE, '.cargo', 'bin', 'cargo.exe'),
    ['test', '-q', '-p', 'shikisha-core', '--lib', 'live_board', '--', '--ignored', '--nocapture'],
    { cwd: ROOT, encoding: 'utf8', env: { ...process.env, SHIKISHA_TEST_BOARD: BOARD, SHIKISHA_TEST_CODE: pcCode } });
  check(/test result: ok\. 1 passed/.test(cargo.stdout), 'the settings\' own calls pair, ask a ticket, a status and a phone code: ' + (cargo.stdout + cargo.stderr).split('\n').filter((l) => /panicked|result|live/.test(l)).join(' | '));

  console.log('7. the board\'s key in a link pairs nobody');
  const token = wsl(`cat ${HOME}/data/remote-token`).stdout.trim();
  check(token.length >= 16, 'the board has its key');
  const byKey = await fetch(`${BOARD}/?t=${token}`, { redirect: 'manual', headers: { 'User-Agent': 'Mozilla/5.0 (Android)' } });
  const byKeyCookies = byKey.headers.getSetCookie ? byKey.headers.getSetCookie() : [];
  const byKeyHtml = await byKey.text();
  check(byKey.status === 403 && !byKeyCookies.some((c) => c.startsWith('rk=')) && byKeyHtml.includes('shikisha-server pair'), 'a new device with the key is told to pair by a code: ' + byKey.status);
  const printed = wsl(`cat ${HOME}/out.log`).stdout;
  check(printed.includes('shikisha-server pair') && !printed.includes(token), 'the address printed carries no key, and says how to add a device');

  console.log('8. guessing is stopped');
  const kept = wsl(`SHIKISHA_HOME=${HOME} ${SERVER} pair`).stdout.match(/[A-Z0-9]{4}-[A-Z0-9]{4}/)[0];
  for (let i = 0; i < 10; i++) await post('/pair', { code: 'AAAA-AAAA', name: 'x' });
  const stopped = await post('/pair', { code: kept, name: 'x' });
  check(stopped.json && stopped.json.why === 'too_many', 'after 10 wrong codes, even a right one waits: ' + JSON.stringify(stopped.json));
} catch (e) {
  failures += 1;
  console.error('stopped: ' + (e.stack || e));
} finally {
  server.kill();
  wsl(`pkill -f '[s]k-target/debug/shikisha-server'; rm -rf ${HOME}`);
}
console.log(failures ? `${failures} failed` : 'all passed');
process.exit(failures ? 1 : 0);

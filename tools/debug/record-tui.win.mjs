/**
 * The recordings of tools/debug/record-tui.py, made on a MicroVM of their own
 * so they are what the Linux builds of the CLIs write -- the ones a server or
 * a MicroVM runs -- and nobody's account is in them: the CLIs are installed
 * fresh there and never signed in, so what they draw is their first screens.
 *
 *     node tools/debug/record-tui.win.mjs
 *
 * Writes target/tty/<cli>.bin. Needs E2B_API_TOKEN in .private/.env. One
 * machine for a few minutes, deleted on the way out; no AI turn is asked for.
 */
import fs from 'node:fs';
import path from 'node:path';
import { spawnSync } from 'node:child_process';

const ROOT = path.resolve(import.meta.dirname, '..', '..');
const MAIN = path.dirname(spawnSync('git', ['-C', ROOT, 'rev-parse', '--path-format=absolute', '--git-common-dir'], { encoding: 'utf8' }).stdout.trim());
const dotenv = Object.fromEntries(fs.readFileSync(path.join(MAIN, '.private', '.env'), 'utf8')
  .split(/\r?\n/).map((l) => l.trim()).filter((l) => l && !l.startsWith('#') && l.includes('='))
  .map((l) => [l.slice(0, l.indexOf('=')).trim(), l.slice(l.indexOf('=') + 1).trim().replace(/^"|"$/g, '')]));
const KEY = dotenv.E2B_API_TOKEN;
if (!KEY) { console.error('E2B_API_TOKEN is needed in .private/.env'); process.exit(2); }
const { Sandbox } = await import('file://' + path.join(ROOT, 'target', 'e2b-sdk', 'node_modules', 'e2b', 'dist', 'index.mjs').replace(/\\/g, '/'));

// The CLIs want Node.js 22 or newer, which the machine's own may not be
const SETUP = [
  'cd "$HOME" && curl -fsSL https://nodejs.org/dist/v22.12.0/node-v22.12.0-linux-x64.tar.xz | tar -xJ',
  'export PATH="$HOME/node-v22.12.0-linux-x64/bin:$HOME/.npm-global/bin:$PATH"',
  'node --version',
  'npm i -g --prefix "$HOME/.npm-global" @openai/codex @google/gemini-cli 2>&1 | tail -3',
  // Claude Code comes as a program of its own, from its own installer
  'curl -fsSL https://claude.ai/install.sh | bash >/dev/null 2>&1',
  'export PATH="$HOME/.local/bin:$PATH"',
  'for c in claude codex gemini; do $c --version 2>&1 | head -1; done',
].join('\n');

const box = await Sandbox.create('base', { apiKey: KEY, timeoutMs: 20 * 60 * 1000, metadata: { shikisha: '1', project: 'record-tui' } });
console.log('machine ' + box.sandboxId);
try {
  const run = async (cmd, timeoutMs = 600000) => {
    const r = await box.commands.run(cmd, { timeoutMs }).catch((e) => e.result || { stdout: '', stderr: String(e), exitCode: 1 });
    return (r.stdout + r.stderr).trim();
  };
  console.log('installing the CLIs...');
  await box.files.write('/home/user/setup.sh', SETUP);
  console.log(await run('bash /home/user/setup.sh', 900000));
  await box.files.write('/home/user/tools/debug/record-tui.py', fs.readFileSync(path.join(ROOT, 'tools', 'debug', 'record-tui.py'), 'utf8'));
  console.log(await run('cd /home/user && PATH="$HOME/.local/bin:$HOME/node-v22.12.0-linux-x64/bin:$HOME/.npm-global/bin:$PATH" python3 tools/debug/record-tui.py', 300000));
  const out = path.join(ROOT, 'target', 'tty');
  fs.mkdirSync(out, { recursive: true });
  for (const cli of ['claude', 'codex', 'gemini']) {
    const bytes = await box.files.read(`/home/user/target/tty/${cli}.bin`, { format: 'bytes' }).catch(() => null);
    if (!bytes) { console.log(`${cli}: nothing recorded`); continue; }
    fs.writeFileSync(path.join(out, `${cli}.bin`), Buffer.from(bytes));
    console.log(`${cli}: ${bytes.length} bytes`);
  }
} finally {
  await box.kill().catch(() => {});
  console.log('machine deleted');
}

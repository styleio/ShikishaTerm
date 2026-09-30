/**
 * A stand-in for a full-screen program that wants the mouse wheel as arrow
 * keys, and writes down every byte it is given.
 *
 *     node tools/debug/alt-scroll-tui.mjs <log file> [--app] [--no-ask]
 *
 * It goes onto the alternate screen and asks for xterm alternate scroll
 * (CSI ? 1007 h) -- what Codex does when it opens its transcript. `--app`
 * also turns on application cursor keys (CSI ? 1 h), so the arrows it is
 * given arrive as ESC O A rather than ESC [ A. `--no-ask` stays on the
 * alternate screen without asking, the case where the wheel must NOT turn
 * into keys. Used by `wheel-keys.win.mjs`; run it in a tab by hand to watch
 * what a turn of the wheel sends.
 */
import fs from 'node:fs';

const [log, ...flags] = process.argv.slice(2);
if (!log) {
  console.error('usage: alt-scroll-tui.mjs <log file> [--app] [--no-ask]');
  process.exit(2);
}
fs.writeFileSync(log, '');
let asks = '\x1b[?1049h';
if (!flags.includes('--no-ask')) asks += '\x1b[?1007h';
if (flags.includes('--app')) asks += '\x1b[?1h';
process.stdout.write(asks + '\x1b[H\x1b[2Jalt-scroll stand-in ' + flags.join(' ') + '\r\n');
if (process.stdin.isTTY) process.stdin.setRawMode(true);
process.stdin.on('data', (d) => {
  fs.appendFileSync(log, d.toString('latin1'));
  process.stdout.write('got ' + JSON.stringify(d.toString('latin1')) + '\r\n');
});

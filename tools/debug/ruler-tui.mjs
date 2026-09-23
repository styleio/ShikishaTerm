/**
 * A full screen that says what size it believes it has, for testing what
 * happens to a terminal when its size changes under it.
 *
 *     node tools/debug/ruler-tui.mjs [--lazy]
 *
 * It takes the alternate screen, the way a full-screen program does, and
 * draws every row it has: "R07" then dots out to the right edge and a "|" on
 * the last column, with one row in the middle saying C= (columns), R= (rows),
 * D= (how many times it has drawn) and V or Z (live or lazy). So a
 * picture of it answers three questions at once -- how wide it thinks it is,
 * how tall, and whether what is on screen is one drawing or two overlaid.
 *
 * Without --lazy it redraws whenever it is told the size changed, which is
 * what a well-behaved program does. With --lazy it draws once and then never
 * again unless a key is pressed: a program that only draws when it has
 * something to say. Between them they separate "the program did not redraw"
 * from "we did not tell it", which look identical on screen.
 *
 * Quit with q. Nothing here ships.
 */
const lazy = process.argv.includes('--lazy');
const out = process.stdout;

const ESC = '\u001b';
let drawn = 0;

function draw() {
  drawn++;
  const cols = out.columns || 80;
  const rows = out.rows || 24;
  let s = ESC + '[2J';
  for (let r = 0; r < rows; r++) {
    const head = 'R' + String(r).padStart(2, '0');
    // Short enough to fit a pane a screen has barely any room for: spelled
    // out, the one row that says the size was the one row that got cut off,
    // and the reading said the program had drawn nothing at all
    const middle = r === Math.floor(rows / 2)
      ? ` C=${cols} R=${rows} D=${drawn} ${lazy ? 'Z' : 'V'} ` : '';
    const body = head + middle;
    const pad = Math.max(0, cols - body.length - 1);
    s += ESC + '[' + (r + 1) + ';1H' + body + '.'.repeat(pad) + '|';
  }
  out.write(s);
}

out.write(ESC + '[?1049h' + ESC + '[?25l');
draw();
if (!lazy) out.on('resize', draw);

if (process.stdin.isTTY) process.stdin.setRawMode(true);
process.stdin.resume();
process.stdin.on('data', (b) => {
  if (String(b) === 'q') {
    out.write(ESC + '[?25h' + ESC + '[?1049l');
    process.exit(0);
  }
  draw();
});

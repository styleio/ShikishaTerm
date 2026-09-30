// A face drawn from a name: the "beam" avatar of boring-avatars 2.0.4
// (commit d0ff2582), src/lib/components/avatar-beam.tsx and
// src/lib/utilities.ts, rewritten as one plain function for the board.
// MIT, Copyright (c) 2021 boringdesigners: the license is in LICENSE beside
// this file and in THIRD-PARTY-NOTICES.txt.
//
// What changed from the original, and nothing else:
// - React is gone: the drawing comes back as an SVG string.
// - No mask. The original cut the circle with a mask whose id had to be
//   unique on the page; the board is rebuilt several times a second, so the
//   caller rounds the drawing with CSS instead (clip-path: circle(50%)).
// - The color of the eyes and mouth is not black or white written in here:
//   the caller hands the dark and the light ink, and the one that reads on
//   the body's color is picked the same way the original picked.
//
// The same name and the same colors give the same face as the original.
// On the board the colors are --face1 to --face5 and the inks --face-ink and
// --face-ink-light (docs/design/STYLEGUIDE.md, "A tab's face").

function beamFace(name, colors, ink) {
  const SIZE = 36;

  // A 32-bit hash of the name, made positive
  let n = 0;
  for (let i = 0; i < name.length; i++) {
    n = ((n << 5) - n) + name.charCodeAt(i);
    n = n & n;
  }
  n = Math.abs(n);

  const digit = (place) => Math.floor((n / Math.pow(10, place)) % 10);
  const even = (place) => digit(place) % 2 === 0;
  // A value below `range`, turned negative when the digit at `place` is even
  const unit = (range, place) => {
    const v = n % range;
    return place && even(place) ? -v : v;
  };
  const pick = (k) => colors[k % colors.length];
  // The ink that reads on a #rrggbb color (YIQ brightness)
  const inkOn = (hex) => {
    const h = hex.replace(/^#/, '');
    const r = parseInt(h.slice(0, 2), 16);
    const g = parseInt(h.slice(2, 4), 16);
    const b = parseInt(h.slice(4, 6), 16);
    return (r * 299 + g * 587 + b * 114) / 1000 >= 128 ? ink[0] : ink[1];
  };

  const preX = unit(10, 1);
  const bodyX = preX < 5 ? preX + SIZE / 9 : preX;
  const preY = unit(10, 2);
  const bodyY = preY < 5 ? preY + SIZE / 9 : preY;
  const body = pick(n);
  const face = inkOn(body);
  const back = pick(n + 13);
  const bodyTurn = unit(360);
  const bodyScale = 1 + unit(SIZE / 12) / 10;
  const mouthOpen = even(2);
  const round = even(1);
  const eyeSpread = unit(5);
  const mouthSpread = unit(3);
  const faceTurn = unit(10, 3);
  const faceX = bodyX > SIZE / 6 ? bodyX / 2 : unit(8, 1);
  const faceY = bodyY > SIZE / 6 ? bodyY / 2 : unit(7, 2);

  const mouth = mouthOpen
    ? `<path d="M15 ${19 + mouthSpread}c2 1 4 1 6 0" stroke="${face}" fill="none" stroke-linecap="round"/>`
    : `<path d="M13,${19 + mouthSpread} a1,0.75 0 0,0 10,0" fill="${face}"/>`;
  const eye = (x) => `<rect x="${x}" y="14" width="1.5" height="2" rx="1" stroke="none" fill="${face}"/>`;

  return `<svg viewBox="0 0 ${SIZE} ${SIZE}" fill="none" xmlns="http://www.w3.org/2000/svg">`
    + `<rect width="${SIZE}" height="${SIZE}" fill="${back}"/>`
    + `<rect x="0" y="0" width="${SIZE}" height="${SIZE}"`
    + ` transform="translate(${bodyX} ${bodyY}) rotate(${bodyTurn} ${SIZE / 2} ${SIZE / 2}) scale(${bodyScale})"`
    + ` fill="${body}" rx="${round ? SIZE : SIZE / 6}"/>`
    + `<g transform="translate(${faceX} ${faceY}) rotate(${faceTurn} ${SIZE / 2} ${SIZE / 2})">`
    + mouth + eye(14 - eyeSpread) + eye(20 + eyeSpread)
    + `</g></svg>`;
}

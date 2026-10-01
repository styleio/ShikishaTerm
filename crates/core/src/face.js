// A tab's face: one of the robots the person conducts, drawn from a name.
//
// The same robots as the site's pictures -- a round coloured head, a dark
// screen for a face with two light eyes on it, an antenna, ears -- drawn flat,
// and each holding the instrument it plays in the orchestra. The name picks
// every part, so the same name is the same player wherever it appears.
//
//   name    the seed (on the board: the desk's id and the tab's id)
//   heads   the colours a head is painted in
//   backs   the pale colours behind it
//   ink     [dark, light]: the screen, the antenna and the instruments' lines
//           in the dark one, the eyes in the light one
//   small   drawn for 14px: the instrument and the ears are left out, which
//           at that size are a smudge, and the head is drawn larger
//   pair    [head, ground]: indexes into heads and backs chosen by the
//           caller, so the tabs side by side on a board each wear their own
//           (see cfPair in shell.rs). Left out, the name picks them
//
// Returns an SVG string on a 36 x 36 square. The caller rounds it (CSS
// clip to a circle): the board is rebuilt several times a second, and an
// SVG clip would need an id unique on the page.

function playerFace(name, heads, backs, ink, small, pair) {
  // FNV-1a over the name, then a small generator from it: each part takes
  // its own draw, so one part's choice does not decide another's
  let h = 0x811c9dc5;
  for (let i = 0; i < name.length; i++) {
    h ^= name.charCodeAt(i);
    h = Math.imul(h, 0x01000193) >>> 0;
  }
  const next = () => {
    h = (h + 0x6d2b79f5) >>> 0;
    let t = h;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
  const pick = (list) => list[Math.floor(next() * list.length)];
  const between = (lo, hi) => lo + next() * (hi - lo);
  const [dark, light] = ink;

  // Colours: a head, a pale ground of another colour, and an accent from the
  // rest of the heads for what it holds
  // Drawn whether or not a pair is given, so a pair changes the colours and
  // nothing else of the face
  const drawnHead = Math.floor(next() * heads.length);
  const drawnBack = Math.floor(next() * backs.length);
  const headAt = pair ? pair[0] : drawnHead;
  const head = heads[headAt];
  // The pale ground of the same family would wash the head out; the lists
  // are kept in the same order of families, so the same index is the same one
  const back = backs[pair ? pair[1] : drawnBack === headAt ? (drawnBack + 1) % backs.length : drawnBack];
  const accent = heads[(headAt + 1 + Math.floor(next() * (heads.length - 1))) % heads.length];
  const shade = darken(head, 0.16);

  const tilt = small ? between(-6, 6) : between(-11, 11);
  const shift = small ? 0 : between(-1.5, 1.5);

  // The head: rounder or squarer, and a body under it the circle cuts off
  const square = next() < 0.4;
  const top = small ? 6 : 8;
  const headBox = small ? { x: 4, y: top, w: 28, h: 25 } : { x: 7, y: top, w: 22, h: 20 };
  const headR = square ? headBox.w * 0.3 : headBox.w * 0.46;
  const cx = headBox.x + headBox.w / 2;

  // The antenna: a ball on a stem, a bent stem, two, or a short flat one
  const antennaKind = pick(["ball", "bent", "two", "stub"]);
  const tip = next() < 0.5 ? accent : dark;
  const stemW = small ? 1.6 : 1.2;
  let antenna = "";
  if (antennaKind === "ball") {
    antenna = `<path d="M${cx} ${top + 1}V${top - 4}" stroke="${dark}" stroke-width="${stemW}" stroke-linecap="round"/>`
      + `<circle cx="${cx}" cy="${top - 4.6}" r="${small ? 2.4 : 1.9}" fill="${tip}"/>`;
  } else if (antennaKind === "bent") {
    antenna = `<path d="M${cx} ${top + 1}V${top - 2.5}l3 -2.5" stroke="${dark}" stroke-width="${stemW}" stroke-linecap="round" stroke-linejoin="round" fill="none"/>`
      + `<circle cx="${cx + 3.3}" cy="${top - 5.2}" r="${small ? 2.2 : 1.7}" fill="${tip}"/>`;
  } else if (antennaKind === "two") {
    antenna = `<path d="M${cx - 3} ${top + 1}l-2 -5M${cx + 3} ${top + 1}l2 -5" stroke="${dark}" stroke-width="${stemW}" stroke-linecap="round"/>`
      + `<circle cx="${cx - 5.1}" cy="${top - 4.4}" r="${small ? 1.9 : 1.5}" fill="${tip}"/>`
      + `<circle cx="${cx + 5.1}" cy="${top - 4.4}" r="${small ? 1.9 : 1.5}" fill="${tip}"/>`;
  } else {
    antenna = `<rect x="${cx - 2.5}" y="${top - 3}" width="5" height="4" rx="1.5" fill="${dark}"/>`;
  }

  // Ears: a disc on each side, sometimes with a dot of the accent in it
  const ears = !small && next() < 0.75;
  const earDot = next() < 0.5;
  const earY = headBox.y + headBox.h * 0.5;
  const ear = (x) => `<circle cx="${x}" cy="${earY}" r="2.6" fill="${shade}"/>`
    + (earDot ? `<circle cx="${x}" cy="${earY}" r="1.1" fill="${accent}"/>` : "");

  // The screen it has for a face, and the eyes on it
  const screenKind = pick(["pill", "wide", "round"]);
  const sw = headBox.w * (screenKind === "round" ? 0.62 : 0.74);
  const sh = headBox.h * (screenKind === "wide" ? 0.42 : 0.5);
  const sx = cx - sw / 2;
  const sy = headBox.y + headBox.h * (screenKind === "wide" ? 0.3 : 0.24);
  const sr = screenKind === "pill" ? sh / 2 : screenKind === "round" ? sh * 0.48 : sh * 0.3;
  const screen = `<rect x="${sx}" y="${sy}" width="${sw}" height="${sh}" rx="${sr}" fill="${dark}"/>`;

  const ey = sy + sh / 2;
  const gap = sw * between(0.2, 0.27);
  const look = small ? 0 : between(-1, 1);
  const eyeKind = pick(["dot", "tall", "happy", "wink", "big", "sleepy"]);
  const unit = small ? 1.35 : 1;
  const eye = (x, kind) => {
    if (kind === "dot") return `<circle cx="${x}" cy="${ey}" r="${1.4 * unit}" fill="${light}"/>`;
    if (kind === "big") return `<circle cx="${x}" cy="${ey}" r="${2 * unit}" fill="${light}"/>`
      + (small ? "" : `<circle cx="${x + 0.6}" cy="${ey - 0.6}" r="0.6" fill="${dark}"/>`);
    if (kind === "tall") return `<rect x="${x - 0.85 * unit}" y="${ey - 1.8 * unit}" width="${1.7 * unit}" height="${3.6 * unit}" rx="${0.85 * unit}" fill="${light}"/>`;
    if (kind === "sleepy") return `<path d="M${x - 1.6 * unit} ${ey}h${3.2 * unit}" stroke="${light}" stroke-width="${1.2 * unit}" stroke-linecap="round"/>`;
    // A curve up: smiling eyes
    return `<path d="M${x - 1.7 * unit} ${ey + 0.8 * unit}q${1.7 * unit} ${-2.8 * unit} ${3.4 * unit} 0" stroke="${light}" stroke-width="${1.2 * unit}" stroke-linecap="round" fill="none"/>`;
  };
  const eyes = eyeKind === "wink"
    ? eye(cx - gap + look, "dot") + eye(cx + gap + look, "happy")
    : eye(cx - gap + look, eyeKind) + eye(cx + gap + look, eyeKind);

  const robot = `<g transform="translate(${shift} 0) rotate(${tilt} ${cx} 30)">`
    + (small ? "" : `<ellipse cx="${cx}" cy="38" rx="13" ry="9" fill="${shade}"/>`)
    + antenna
    + (ears ? ear(headBox.x) + ear(headBox.x + headBox.w) : "")
    + `<rect x="${headBox.x}" y="${headBox.y}" width="${headBox.w}" height="${headBox.h}" rx="${headR}" fill="${head}"/>`
    + screen + eyes
    + `</g>`;

  return `<svg viewBox="0 0 36 36" fill="none" xmlns="http://www.w3.org/2000/svg">`
    + `<rect width="36" height="36" fill="${back}"/>`
    + robot
    + (small ? "" : instrument(pick(INSTRUMENTS), accent, dark, light, next() < 0.5))
    + `</svg>`;
}

// What a player can hold. Drawn in the lower corner, in front of the body,
// kept inside the circle the face is cut to (radius 18 about 18,18)
const INSTRUMENTS = ["note", "notes", "violin", "trumpet", "drum", "flute", "mic"];

function instrument(kind, accent, dark, light, left) {
  let g = "";
  if (kind === "note") {
    g = `<ellipse cx="23.5" cy="29.5" rx="2.7" ry="2.1" transform="rotate(-20 23.5 29.5)" fill="${dark}"/>`
      + `<path d="M25.7 29V20.5c2 .7 3.3 2 3.7 4" stroke="${dark}" stroke-width="1.4" stroke-linecap="round" fill="none"/>`;
  } else if (kind === "notes") {
    g = `<ellipse cx="21.8" cy="30" rx="2.3" ry="1.8" transform="rotate(-20 21.8 30)" fill="${dark}"/>`
      + `<ellipse cx="27.8" cy="28.6" rx="2.3" ry="1.8" transform="rotate(-20 27.8 28.6)" fill="${dark}"/>`
      + `<path d="M23.7 29.5V22.6l6-1.5V28" stroke="${dark}" stroke-width="1.3" stroke-linejoin="round" fill="none"/>`
      + `<path d="M23.7 22.6l6-1.5" stroke="${dark}" stroke-width="2.4"/>`;
  } else if (kind === "violin") {
    // Upright and leaning back, the bow across it
    g = `<g transform="rotate(28 25 27)">`
      + `<path d="M25 16.5v8" stroke="${dark}" stroke-width="1.5" stroke-linecap="round"/>`
      + `<circle cx="25" cy="16" r="1.3" fill="${dark}"/>`
      + `<path d="M25 23.2c-2.6 0-3.6 1.4-3.6 2.7 0 1 .9 1.6.9 2.3s-1.4 1.3-1.4 2.9c0 1.8 1.8 3.1 4.1 3.1s4.1-1.3 4.1-3.1c0-1.6-1.4-2.2-1.4-2.9s.9-1.3.9-2.3c0-1.3-1-2.7-3.6-2.7z" fill="${accent}"/>`
      + `<path d="M25 25.5v6" stroke="${dark}" stroke-width=".8"/>`
      + `</g>`
      + `<path d="M18.5 24.5l13 7.5" stroke="${dark}" stroke-width="1.1" stroke-linecap="round"/>`;
  } else if (kind === "trumpet") {
    g = `<path d="M15 27h7.5" stroke="${dark}" stroke-width="1.7" stroke-linecap="round"/>`
      + `<path d="M22 27l5.5-3.8v7.6z" fill="${accent}" stroke="${accent}" stroke-width="1.6" stroke-linejoin="round"/>`
      + `<path d="M17.5 27v-2.8M19.8 27v-2.8" stroke="${dark}" stroke-width="1.2" stroke-linecap="round"/>`;
  } else if (kind === "drum") {
    g = `<path d="M20 26.5v4.5c0 1.4 2.5 2.5 5.5 2.5s5.5-1.1 5.5-2.5v-4.5z" fill="${accent}"/>`
      + `<ellipse cx="25.5" cy="26.5" rx="5.5" ry="2" fill="${light}"/>`
      + `<path d="M20 26.5c0 1.1 2.5 2 5.5 2s5.5-.9 5.5-2" stroke="${dark}" stroke-width=".6" opacity=".35"/>`
      + `<path d="M21 20.5l3.2 5M30.5 20.5l-3.2 5" stroke="${dark}" stroke-width="1.3" stroke-linecap="round"/>`
      + `<circle cx="21" cy="20.5" r="1" fill="${dark}"/><circle cx="30.5" cy="20.5" r="1" fill="${dark}"/>`;
  } else if (kind === "flute") {
    g = `<path d="M17 32l15.5 -8.5" stroke="${accent}" stroke-width="2.6" stroke-linecap="round"/>`
      + `<circle cx="24" cy="28.2" r=".65" fill="${dark}"/><circle cx="26.6" cy="26.8" r=".65" fill="${dark}"/>`
      + `<circle cx="29.2" cy="25.4" r=".65" fill="${dark}"/>`;
  } else {
    // A singer microphone
    g = `<path d="M26 26.5l-3 6" stroke="${dark}" stroke-width="2.2" stroke-linecap="round"/>`
      + `<circle cx="27" cy="24" r="3.3" fill="${accent}"/>`
      + `<path d="M25.2 22.6l3.6 2.8M24.9 24.6l2.8 2.1M26.5 21l2.7 2" stroke="${dark}" stroke-width=".55" opacity=".45"/>`;
  }
  // Mirrored for the left corner, so not every player holds it on one side
  return left ? `<g transform="translate(36 0) scale(-1 1)">${g}</g>` : g;
}

// A #rrggbb colour toward black by `by` (0..1): the body under a head and its ears
function darken(hex, by) {
  const h = hex.replace(/^#/, "");
  const c = (i) => Math.round(parseInt(h.slice(i, i + 2), 16) * (1 - by)).toString(16).padStart(2, "0");
  return "#" + c(0) + c(2) + c(4);
}

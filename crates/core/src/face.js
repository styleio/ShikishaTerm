// A tab's face: a small alien drawn from a name.
//
// Two colours and a few marks: a soft, slightly lopsided body on a ground of
// another colour, cut by the circle it is shown in, and on it one, two or
// three eyes, sometimes a mouth, sometimes feelers. The name picks every
// part, so the same name is the same alien wherever it appears.
//
//   name    the seed (on the board: the desk's id and the tab's id)
//   colors  the colours a body and its ground are painted in
//   ink     [dark, light]: the eyes and the mouth take the one that reads on
//           the body, the shine in an eye the other
//   small   drawn for 14px: larger eyes, no mouth and no feelers, which at
//           that size are a smudge
//   pair    [body, ground]: indexes into colors chosen by the caller, so the
//           tabs side by side on a board each wear their own (cfPair in
//           shell.rs). Left out, the name picks them
//
// Returns an SVG string on a 36 x 36 square. The caller rounds it (CSS clip
// to a circle): the board is rebuilt several times a second, and an SVG clip
// would need an id unique on the page.

// A name as a 32-bit number (FNV-1a): where every face starts, and what the
// board hashes a tab by when it hands out colours (cfPair in shell.rs)
function faceHash(name) {
  let h = 0x811c9dc5;
  for (let i = 0; i < name.length; i++) {
    h ^= name.charCodeAt(i);
    h = Math.imul(h, 0x01000193) >>> 0;
  }
  return h;
}

function alienFace(name, colors, ink, small, pair) {
  // A small generator from the name's hash: each part takes its own draw, so
  // one part's choice does not decide another's
  let h = faceHash(name);
  const next = () => {
    h = (h + 0x6d2b79f5) >>> 0;
    let t = h;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
  const between = (lo, hi) => lo + next() * (hi - lo);
  const one = (list) => list[Math.floor(next() * list.length)];
  const f = (v) => Math.round(v * 100) / 100;

  // Colours. Drawn whether or not a pair is given, so a pair changes the
  // colours and nothing else of the face
  const drawnBody = Math.floor(next() * colors.length);
  const drawnStep = 1 + Math.floor(next() * (colors.length - 1));
  const bodyAt = pair ? pair[0] : drawnBody;
  const body = colors[bodyAt];
  const back = colors[pair ? pair[1] : (bodyAt + drawnStep) % colors.length];
  const mark = alienInk(body, ink);
  const shine = mark === ink[0] ? ink[1] : ink[0];

  // The body: a ring of points at uneven distances, joined smoothly, set off
  // the middle so the circle cuts it -- what makes it a creature and not a
  // badge. Kept large enough that the eyes always land on it
  const cx = 18 + between(-3, 3);
  const cy = 18 + between(-1, 6);
  const r = small ? between(15, 17) : between(13, 15.5);
  const points = 7;
  const turn = between(0, Math.PI * 2);
  const ring = [];
  for (let i = 0; i < points; i++) {
    const a = turn + (i / points) * Math.PI * 2;
    const d = r * between(0.9, 1.12);
    ring.push([cx + Math.cos(a) * d, cy + Math.sin(a) * d]);
  }
  const blob = alienOutline(ring, f);

  // Feelers: none, one or two, rising from the top of the body
  const feelers = small ? 0 : one([0, 0, 0, 1, 2]);
  let stalks = "";
  for (let i = 0; i < feelers; i++) {
    const side = feelers === 1 ? between(-0.4, 0.4) : (i === 0 ? -1 : 1) * between(0.35, 0.6);
    const sx = cx + side * r * 0.7;
    const sy = cy - r * 0.75;
    const tx = sx + side * between(3, 5);
    const ty = sy - between(5, 7);
    stalks += `<path d="M${f(sx)} ${f(sy)}Q${f(sx + side * 0.5)} ${f(ty + 1)} ${f(tx)} ${f(ty)}" stroke="${body}" stroke-width="2" stroke-linecap="round" fill="none"/>`
      + `<circle cx="${f(tx)}" cy="${f(ty)}" r="2.1" fill="${body}"/>`;
  }

  // The face, near the middle of what shows: one big eye, two, or three
  const fx = (cx + 18) / 2 + between(-1, 1);
  const fy = Math.min(cy, 20) - (small ? 1 : 2);
  const kind = one(["one", "two", "two", "two", "three", "pair-big"]);
  const grow = small ? 1.35 : 1;
  const dot = (x, y, rr) => `<circle cx="${f(x)}" cy="${f(y)}" r="${f(rr * grow)}" fill="${mark}"/>`;
  const shiny = (x, y, rr) => dot(x, y, rr)
    + `<circle cx="${f(x + rr * grow * 0.35)}" cy="${f(y - rr * grow * 0.35)}" r="${f(rr * grow * 0.32)}" fill="${shine}"/>`;
  const tall = (x, y) => `<ellipse cx="${f(x)}" cy="${f(y)}" rx="${f(1.15 * grow)}" ry="${f(1.7 * grow)}" fill="${mark}"/>`;
  let eyes;
  if (kind === "one") {
    eyes = shiny(fx, fy, 3);
  } else if (kind === "pair-big") {
    const gap = 3.6 * grow;
    eyes = shiny(fx - gap, fy, 2) + shiny(fx + gap, fy, 2);
  } else if (kind === "three") {
    const gap = 3.8 * grow;
    eyes = dot(fx - gap, fy + 0.6, 1.2) + dot(fx, fy - 0.8, 1.35) + dot(fx + gap, fy + 0.6, 1.2);
  } else {
    const gap = between(2.6, 3.6) * grow;
    eyes = tall(fx - gap, fy) + tall(fx + gap, fy);
  }

  // A mouth, or not: a small smile, a little round one, a flat line
  const my = fy + (kind === "one" ? 5.5 : 4.5) * (small ? 1.1 : 1);
  const mouthKind = small ? "none" : one(["none", "smile", "smile", "o", "line"]);
  const mouth = mouthKind === "smile"
    ? `<path d="M${f(fx - 2.2)} ${f(my)}q2.2 2 4.4 0" stroke="${mark}" stroke-width="1.1" stroke-linecap="round" fill="none"/>`
    : mouthKind === "o"
      ? `<ellipse cx="${f(fx)}" cy="${f(my + 0.5)}" rx="1.1" ry="1.3" fill="${mark}"/>`
      : mouthKind === "line"
        ? `<path d="M${f(fx - 1.6)} ${f(my + 0.4)}h3.2" stroke="${mark}" stroke-width="1.1" stroke-linecap="round"/>`
        : "";

  const tilt = between(-12, 12);
  return `<svg viewBox="0 0 36 36" fill="none" xmlns="http://www.w3.org/2000/svg">`
    + `<rect width="36" height="36" fill="${back}"/>`
    + stalks
    + `<path d="${blob}" fill="${body}"/>`
    + `<g transform="rotate(${f(tilt)} ${f(fx)} ${f(fy)})">${eyes}${mouth}</g>`
    + `</svg>`;
}

// A closed, smooth outline through `ring` (Catmull-Rom turned into cubic
// curves), as an SVG path
function alienOutline(ring, f) {
  const n = ring.length;
  const at = (i) => ring[(i + n) % n];
  let d = `M${f(ring[0][0])} ${f(ring[0][1])}`;
  for (let i = 0; i < n; i++) {
    const [p0, p1, p2, p3] = [at(i - 1), at(i), at(i + 1), at(i + 2)];
    const c1 = [p1[0] + (p2[0] - p0[0]) / 6, p1[1] + (p2[1] - p0[1]) / 6];
    const c2 = [p2[0] - (p3[0] - p1[0]) / 6, p2[1] - (p3[1] - p1[1]) / 6];
    d += `C${f(c1[0])} ${f(c1[1])} ${f(c2[0])} ${f(c2[1])} ${f(p2[0])} ${f(p2[1])}`;
  }
  return d + "Z";
}

// The ink that reads on a #rrggbb colour, by its brightness
function alienInk(hex, ink) {
  const h = hex.replace(/^#/, "");
  const [r, g, b] = [0, 2, 4].map((i) => parseInt(h.slice(i, i + 2), 16));
  return (r * 299 + g * 587 + b * 114) / 1000 >= 128 ? ink[0] : ink[1];
}

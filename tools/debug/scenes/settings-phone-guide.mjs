/**
 * The Remote access card as the phone page on the site shows it, for
 * tools/debug/settings-shoot.mjs.
 *
 *     node tools/debug/settings-shoot.mjs tools/debug/scenes/settings-phone-guide.mjs
 *
 * The copy served by settings_serve runs no board, so the card would only say
 * it is stopped. What the site needs is the card of a PC already on Tailscale:
 * the scene answers the card's own questions (/api/remote, its devices) as such
 * a PC answers them, with an address of the documentation's own rather than
 * this machine's. The QR is drawn as noise and blurred -- a real one would be a
 * link with a key in it, and a picture on a website must not open anybody's PC.
 */

const scene = '(async () => {'
  + ' const was = window.fetch;'
  + ' window.fetch = (u, o) => {'
  + '   const at = String(u);'
  + '   const json = (j) => Promise.resolve(new Response(JSON.stringify(j), {headers:{"Content-Type":"application/json"}}));'
  + '   if (at.startsWith("/api/remote/clients")) return json({clients: []});'
  + '   if (at.startsWith("/api/remote") && !at.startsWith("/api/remote/qr")) return json({running: true,'
  + '     tailscale: "100.101.102.103", origin: "http://100.101.102.103:8787", kind: "tailscale", https: false});'
  + '   return was(u, o);'
  + ' };'
  + ' current.remote = Object.assign(current.remote || {}, {enabled: true});'
  + ' sel = {desk:null, grp:null, tab:null, global:true, section:"remote"}; render();'
  + ' await new Promise(r => setTimeout(r, 700));'
  // Ticks the box the way a person has ticked it before the card is read
  + ' const on = document.querySelector("#detail .card input[type=checkbox]"); if (on) on.checked = true;'
  + ' const img = document.querySelector("#detail img[src*=\'/api/remote/qr\']");'
  + ' if (img) {'
  + '   const c = document.createElement("canvas"); c.width = c.height = 29; const g = c.getContext("2d");'
  + '   g.fillStyle = "#fff"; g.fillRect(0, 0, 29, 29); g.fillStyle = "#000";'
  + '   let s = 7; const rnd = () => (s = (s * 16807) % 2147483647) / 2147483647;'
  + '   for (let y = 0; y < 29; y++) for (let x = 0; x < 29; x++) if (rnd() < 0.5) g.fillRect(x, y, 1, 1);'
  + '   for (const [fx, fy] of [[0,0],[22,0],[0,22]]) { g.fillStyle = "#000"; g.fillRect(fx, fy, 7, 7);'
  + '     g.fillStyle = "#fff"; g.fillRect(fx+1, fy+1, 5, 5); g.fillStyle = "#000"; g.fillRect(fx+2, fy+2, 3, 3); }'
  + '   img.src = c.toDataURL(); img.style.imageRendering = "pixelated"; img.style.filter = "blur(2.5px)";'
  + ' }'
  + ' document.querySelector("#detail .card").scrollIntoView({block:"start"});'
  + ' await new Promise(r => setTimeout(r, 300));'
  + '})()';

export default {
  config: {
    remote: { enabled: true },
    desks: [{
      name: 'site', id: 'site',
      folders: [{ name: 'site', cwd: 'D:/work/site', tabs: [{ name: 'shell', command: 'cmd' }] }],
    }],
  },
  looks: ['dark'],
  sizes: [['wide', 1280, 1180]],
  scenes: { phone: scene },
};

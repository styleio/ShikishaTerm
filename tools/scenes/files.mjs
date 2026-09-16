/**
 * The file panel's windows, for tools/shoot.mjs.
 *
 * A scene file default-exports:
 *
 *   setup    JavaScript run on the page before every scene. Where the state
 *            the screen draws from is filled in
 *   scenes   name -> the JavaScript that puts that screen up. A scene can be
 *            an object instead, { run, langs, looks, sizes }, to be
 *            photographed in fewer places than the rest
 *   langs    default ["en", "ja"]
 *   looks    default ["dark", "light"]
 *   sizes    default [["wide", 1280, 860], ["phone", 390, 820]]
 *   settle   milliseconds to let the page finish loading. Default 900
 *
 * Every scene runs against a page that has just loaded, so none of them can be
 * upset by the one before it.
 */

// A connection, two folders, and files chosen so that a send replaces one
// thing and brings two new ones
const setup = `
  S = {tabs:[{index:0, kind:"sftp", id:"deploy", name:"deploy"}], active:0};
  F.server = "deploy@prod.example.com";
  F.local.root = "D:/site/dist"; F.local.at = "D:/site/dist";
  F.remote.root = "/var/www/site"; F.remote.at = "/var/www/site";
  F.local.rows = [
    {name:"index.html", dir:false, size:24710, modified:1789000000},
    {name:"app.8f21c4.js", dir:false, size:1462300, modified:1789000000},
    {name:"logo.svg", dir:false, size:3120, modified:1788900000}];
  F.remote.rows = [
    {name:"index.html", dir:false, size:19004, modified:1787000000},
    {name:"app.1a0b7e.js", dir:false, size:1201900, modified:1787000000}];
  "ok"`;

// A comparison, answered the way the app answers it rather than by waiting on
// a server that is not here
const patch = [
  'diff --git a/index.html b/index.html',
  '@@ -3,9 +3,10 @@',
  '   <title>Shinkoku</title>',
  '-  <link rel=stylesheet href=/app.1a0b7e.css>',
  '+  <link rel=stylesheet href=/app.8f21c4.css>',
  '+  <meta name=theme-color content=#101014>',
  ' </head>',
  ' <body>',
  '-  <h1>Welcome</h1>',
  '+  <h1>Welcome back</h1>',
  '   <div id=app></div>',
].join('\n');

const answer = (msg) => 'window.__sftp(' + JSON.stringify(msg) + '); "ok"';

export default {
  setup,
  scenes: {
    // What a send would do, before any of it is done
    send: 'F.local.sel = new Set(["index.html", "app.8f21c4.js", "logo.svg"]);'
      + ' sftpSend("local"); "ok"',
    // The questions about the things that cannot be undone, each naming the
    // machine it is about
    remove: {
      run: 'askQuestion({title:T["sftp.remove.title"], say:T["sftp.remove.say"],'
        + ' what: sftpWhere("remote", rjoin(F.remote.at, "index.html")),'
        + ' label:T["sftp.remove"], danger:true, go(){}}); "ok"',
      looks: ['dark'],
      sizes: [['wide', 1280, 860]],
    },
    // The way in to a comparison, on a name that is on both sides...
    menu: {
      run: 'sftpRowMenu(document.body, "local", F.local.rows[0]); "ok"',
      langs: ['ja'],
      looks: ['dark'],
      sizes: [['wide', 1280, 860]],
    },
    // ...and on one that is not, where there is nothing to hold it against
    menualone: {
      run: 'sftpRowMenu(document.body, "local", F.local.rows[2]); "ok"',
      langs: ['ja'],
      looks: ['dark'],
      sizes: [['wide', 1280, 860]],
    },
    diff: 'openDiff("index.html"); '
      + answer({ act: 'diff', ok: true, name: 'index.html', text: patch }),
    diffsame: {
      run: 'openDiff("index.html"); '
        + answer({ act: 'diff', ok: true, name: 'index.html', text: '' }),
      looks: ['dark'],
      sizes: [['wide', 1280, 860]],
    },
    diffbad: {
      run: 'openDiff("app.8f21c4.js"); window.__sftp({act:"diff", ok:false,'
        + ' name:"app.8f21c4.js",'
        + ' error:T["err.sftp.diff_not_text"].replace("{name}", "app.8f21c4.js")}); "ok"',
      looks: ['dark'],
      sizes: [['wide', 1280, 860]],
    },
  },
};

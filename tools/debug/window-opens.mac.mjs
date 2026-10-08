// Open SHIKISHA-TERM.app as a copy that is nobody's, and say whether its
// window came up drawn: Chromium started inside the app, the board's page
// loaded into it, and the page can talk to the program (window.ipc).
//
//     node tools/debug/window-opens.mac.mjs <SHIKISHA-TERM.app> [out]
//
// It leaves a picture of the window's page (window.png) and the copy's logs in
// [out], so a run on a machine nobody is sitting at -- CI -- can still be
// looked at. Exits non-zero if the window did not come up.
//
// The copy has a folder of its own (SHIKISHA_HOME) holding its settings and
// state, and its own ports, so it touches nothing of a copy somebody uses.

import { spawn } from 'node:child_process'
import fs from 'node:fs'
import net from 'node:net'
import os from 'node:os'
import path from 'node:path'

const app = process.argv[2]
const out = path.resolve(process.argv[3] ?? 'window-opens')
if (!app || !fs.existsSync(path.join(app, 'Contents/MacOS/SHIKISHA-TERM'))) {
  console.error('usage: node tools/debug/window-opens.mac.mjs <SHIKISHA-TERM.app> [out]')
  process.exit(2)
}
fs.mkdirSync(out, { recursive: true })

const freePort = () =>
  new Promise((resolve, reject) => {
    const s = net.createServer()
    s.on('error', reject)
    s.listen(0, '127.0.0.1', () => {
      const { port } = s.address()
      s.close(() => resolve(port))
    })
  })
const sleep = (ms) => new Promise((r) => setTimeout(r, ms))

const home = fs.mkdtempSync(path.join(os.tmpdir(), 'sk-window-'))
const work = path.join(home, 'work')
fs.mkdirSync(path.join(home, 'config'), { recursive: true })
fs.mkdirSync(work, { recursive: true })
const cdp = await freePort()
fs.writeFileSync(
  path.join(home, 'config', 'config.json'),
  JSON.stringify(
    {
      language: 'en',
      agent_hooks: { 'Claude Code': 'off', 'Codex CLI': 'off', 'Gemini CLI': 'off' },
      remote: { enabled: false },
      desks: [{ name: 'Check', id: 'check', folders: [{ cwd: work, tabs: [{ name: 'shell', id: 'shell', command: '/bin/sh' }] }] }],
    },
    null,
    2,
  ),
)

// Its own environment: nothing of an agent's session is handed on to the
// programs the copy starts
const env = Object.fromEntries(Object.entries(process.env).filter(([k]) => !/^(CLAUDE|ANTHROPIC|SHIKISHA)/.test(k)))
env.SHIKISHA_HOME = home
env.SHIKISHA_CHROMIUM_ARGS = `--remote-debugging-port=${cdp}`
const child = spawn(path.join(app, 'Contents/MacOS/SHIKISHA-TERM'), [], { env, cwd: work, stdio: ['ignore', 'pipe', 'pipe'] })
let said = ''
child.stdout.on('data', (d) => (said += d))
child.stderr.on('data', (d) => (said += d))
let ended = null
child.on('exit', (code, signal) => (ended = { code, signal }))

const verdict = { ok: false, steps: [] }
const step = (name, ok, detail) => {
  verdict.steps.push({ name, ok, detail })
  console.log(`${ok ? 'ok  ' : 'FAIL'} ${name}${detail ? ': ' + detail : ''}`)
}

async function pageTarget() {
  const until = Date.now() + 90_000
  while (Date.now() < until) {
    if (ended) return null
    try {
      const list = await (await fetch(`http://127.0.0.1:${cdp}/json/list`)).json()
      const page = list.find((t) => t.type === 'page' && /^http:\/\/127\.0\.0\.1:\d+\//.test(t.url))
      if (page) return page
    } catch {}
    await sleep(500)
  }
  return null
}

function session(wsUrl) {
  const ws = new WebSocket(wsUrl)
  let next = 1
  const waiting = new Map()
  ws.onmessage = (m) => {
    const msg = JSON.parse(m.data)
    if (msg.id && waiting.has(msg.id)) {
      waiting.get(msg.id)(msg)
      waiting.delete(msg.id)
    }
  }
  const opened = new Promise((resolve, reject) => {
    ws.onopen = resolve
    ws.onerror = reject
  })
  const call = (method, params = {}) =>
    new Promise((resolve) => {
      const id = next++
      waiting.set(id, resolve)
      ws.send(JSON.stringify({ id, method, params }))
    })
  return { opened, call, close: () => ws.close() }
}

try {
  const target = await pageTarget()
  step('Chromium started and the board page is open', !!target, target?.url ?? (ended ? `the program ended (${JSON.stringify(ended)})` : 'no page in 90 s'))
  if (target) {
    const s = session(target.webSocketDebuggerUrl)
    await s.opened
    const evaluate = async (expression) => (await s.call('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true })).result?.result?.value
    // The page is ready once it has drawn its body; a fresh copy may still be
    // settling for a moment
    let ready = false
    for (let i = 0; i < 60 && !ready; i++) {
      ready = (await evaluate('document.readyState === "complete" && document.body && document.body.children.length > 0')) === true
      if (!ready) await sleep(500)
    }
    step('the page finished loading and drew something', ready)
    const ipc = await evaluate('typeof window.ipc === "object" && typeof window.ipc.postMessage === "function"')
    step('the page can talk to the program (window.ipc)', ipc === true)
    const size = await evaluate('JSON.stringify([innerWidth, innerHeight])')
    step('the page has a size', !!size && size !== '[0,0]', size)
    // The bar is in the page from the start but empty: it is drawn on the first
    // state the program sends, which may come after the page has loaded. Read it
    // once it has been drawn (drawTitle marks it with what it drew from)
    let drawn = false
    for (let i = 0; i < 60 && !drawn; i++) {
      drawn = (await evaluate('(() => { const b = document.getElementById("titlebar"); return !!b && !!b.dataset.key; })()')) === true
      if (!drawn) await sleep(250)
    }
    const frame = await evaluate(
      '(() => { const b = document.getElementById("titlebar"); return !!b && b.classList.contains("macframe") && !b.querySelector(".wbtn"); })()',
    )
    step("the bar leaves room for the Mac's own buttons and draws none", drawn && frame === true, drawn ? '' : 'the bar was not drawn in 15 s')
    // Typing reaches the program in the terminal, and what it prints comes
    // back: the whole way round, through the page, the program and the pty
    await evaluate('(() => { const k = document.getElementById("kbd"); if (k) k.focus(); return !!k; })()')
    await s.call('Input.insertText', { text: 'echo shikisha-$((6*7))' })
    for (const type of ['keyDown', 'keyUp']) {
      await s.call('Input.dispatchKeyEvent', { type, key: 'Enter', code: 'Enter', windowsVirtualKeyCode: 13 })
    }
    let echoed = false
    for (let i = 0; i < 40 && !echoed; i++) {
      echoed = (await evaluate('document.body.innerText.includes("shikisha-42")')) === true
      if (!echoed) await sleep(250)
    }
    step('typing in the terminal runs a command and shows what it printed', echoed)
    await sleep(2000)
    const shot = await s.call('Page.captureScreenshot', { format: 'png' })
    if (shot.result?.data) {
      fs.writeFileSync(path.join(out, 'window.png'), Buffer.from(shot.result.data, 'base64'))
      step('a picture of the page was taken', true, path.join(out, 'window.png'))
    } else {
      step('a picture of the page was taken', false, JSON.stringify(shot.error ?? {}))
    }
    s.close()
  }
  verdict.ok = verdict.steps.every((s) => s.ok)
} finally {
  if (!ended) child.kill('SIGTERM')
  await sleep(3000)
  if (!ended) child.kill('SIGKILL')
  fs.writeFileSync(path.join(out, 'output.txt'), said)
  for (const d of ['logs']) {
    const from = path.join(home, d)
    if (fs.existsSync(from)) fs.cpSync(from, path.join(out, d), { recursive: true })
  }
  fs.writeFileSync(path.join(out, 'verdict.json'), JSON.stringify(verdict, null, 2))
  fs.rmSync(home, { recursive: true, force: true })
}
process.exit(verdict.ok ? 0 : 1)

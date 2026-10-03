// Exercise the served board's real polling/authentication code with delayed
// replies and a controlled clock. No browser, server or account is needed.
// node --test tools/check-remote-auth.mjs
import fs from 'node:fs';
import vm from 'node:vm';
import assert from 'node:assert/strict';
import {test} from 'node:test';

const source = fs.readFileSync(new URL('../crates/core/src/shell.rs', import.meta.url), 'utf8').replace(/\r\n/g, '\n');
const start = source.indexOf('  // Fallback poll');
const end = source.indexOf('\n}\n\n// ── Screen relay', start);
assert.ok(start >= 0 && end > start, 'the real polling code was not found');
const polling = source.slice(start, end);
const T = JSON.parse(fs.readFileSync(new URL('../lang/en.json', import.meta.url), 'utf8'));
const flush = () => new Promise(resolve => setImmediate(resolve));
const response = (status, body = '', after = null) => ({status, ok: status >= 200 && status < 300,
  text: async () => body, json: async () => ({}), headers: {get: () => after}});

function viewer(answers = ['correct']) {
  const requests = [], prompts = [], alerts = [], states = [], nets = [];
  let now = 1000, tick, reloads = 0;
  const context = vm.createContext({
    T, TOKEN: '', MOVE_CODE: '', wsUp: false, remoteCut: false, moving: null, downSince: 0,
    Date: {now: () => now}, encodeURIComponent,
    fetch: (url, options) => new Promise((resolve, reject) => requests.push({url, options, resolve, reject})),
    prompt: message => { prompts.push(message); return answers.shift() ?? null; },
    alert: message => alerts.push(message),
    location: {reload: () => reloads++},
    applyState: state => states.push(state),
    showNet: state => nets.push(state),
    cutNow: () => { context.remoteCut = true; },
    setInterval: (fn, delay) => { assert.equal(delay, 1500); tick = fn; },
  });
  vm.runInContext(polling, context);
  return {context, requests, prompts, alerts, states, nets,
    get reloads() { return reloads; },
    tick: async () => { void tick(); await flush(); },
    advance: ms => { now += ms; },
    reply: async (index, status, body, after) => { requests[index].resolve(response(status, body, after)); await flush(); },
  };
}

test('slow state and auth replies cannot open a second password prompt', async () => {
  const v = viewer();
  await v.tick(); await v.tick();
  assert.equal(v.requests.length, 1);
  await v.reply(0, 403, 'password');
  assert.equal(v.prompts.length, 1);
  assert.match(v.requests[1].url, /^auth\?/);
  assert.equal(JSON.parse(v.requests[1].options.body).password, 'correct');
  assert.ok(!v.requests[1].url.includes('correct'));
  await v.tick(); await v.tick();
  assert.equal(v.requests.length, 2);
  assert.equal(v.prompts.length, 1);
  await v.reply(1, 200, 'ok');
  assert.equal(v.reloads, 1);
  assert.deepEqual(v.alerts, []);
});

test('a wrong password stays locked and a later correct answer can retry', async () => {
  const v = viewer(['wrong', 'correct']);
  await v.reply(0, 403, 'password');
  await v.reply(1, 403, 'wrong');
  assert.deepEqual(v.alerts, [T['tui.remote.password_wrong']]);
  assert.equal(v.reloads, 0);
  assert.deepEqual(v.states, []);
  await v.tick();
  await v.reply(2, 403, 'password');
  await v.reply(3, 200, 'ok');
  assert.equal(v.reloads, 1);
});

for (const status of [404, 500, 'network']) {
  test(`${status} during authentication is not called a wrong password`, async () => {
    const v = viewer();
    await v.reply(0, 403, 'password');
    if (status === 'network') { v.requests[1].reject(new Error('offline')); await flush(); }
    else await v.reply(1, status, 'unavailable');
    assert.deepEqual(v.alerts, [T['tui.remote.password_failed']]);
    assert.equal(v.reloads, 0);
    await v.tick();
    assert.equal(v.requests.length, 3, 'a failed request kept the poll locked');
  });
}

for (const why of ['cut', 'forbidden']) {
  test(`an authentication refusal (${why}) disconnects without a password error`, async () => {
    const v = viewer();
    await v.reply(0, 403, 'password');
    await v.reply(1, 403, why);
    assert.equal(v.context.remoteCut, true);
    assert.deepEqual(v.alerts, []);
    await v.tick();
    assert.equal(v.requests.length, 2);
  });
}

for (const state of ['wsUp', 'remoteCut', 'moving']) {
  test(`a late password challenge is discarded after ${state}`, async () => {
    const v = viewer();
    // The headers can arrive before a connection succeeds, and the body after.
    let body;
    v.requests[0].resolve({...response(403), text: () => new Promise(r => { body = r; })});
    await flush();
    v.context[state] = true;
    body('password'); await flush();
    assert.equal(v.prompts.length, 0);
    assert.equal(v.requests.length, 1);
  });
}

test('an authentication result cannot reopen a session cut while it was pending', async () => {
  const v = viewer();
  await v.reply(0, 403, 'password');
  v.context.remoteCut = true;
  await v.reply(1, 200, 'ok');
  assert.equal(v.reloads, 0);
});

test('the retry delay is respected without prompting or sending more requests', async () => {
  const v = viewer();
  await v.reply(0, 403, 'password');
  await v.reply(1, 429, 'wait', '5');
  assert.deepEqual(v.alerts, [T['tui.remote.password_wait'].replaceAll('{n}', 5)]);
  v.advance(4999); await v.tick();
  assert.equal(v.requests.length, 2);
  v.advance(1); await v.tick();
  assert.equal(v.requests.length, 3);
});

for (const answer of [null, '']) {
  test(`dismissing the password prompt (${JSON.stringify(answer)}) sends no password`, async () => {
    const v = viewer([answer]);
    await v.reply(0, 403, 'password');
    assert.equal(v.requests.length, 1);
    assert.equal(v.reloads, 0);
    await v.tick();
    assert.equal(v.requests.length, 2);
  });
}

test('a failed state request can recover on the next poll', async () => {
  const v = viewer();
  v.requests[0].reject(new Error('offline')); await flush();
  await v.tick();
  await v.reply(1, 200);
  assert.equal(v.states.length, 1);
  assert.deepEqual(v.prompts, []);
});

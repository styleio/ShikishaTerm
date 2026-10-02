import {test} from 'node:test';
import assert from 'node:assert/strict';
import {connectCdp} from './chrome.mjs';

class Socket extends EventTarget {
  static last;
  constructor() { super(); Socket.last = this; queueMicrotask(() => this.dispatchEvent(new Event('open'))); }
  reply(message) { this.dispatchEvent(new MessageEvent('message', {data:JSON.stringify(message)})); }
  send(raw) {
    const {id, method, params} = JSON.parse(raw);
    if (method === 'hang') return;
    queueMicrotask(() => this.reply(method === 'bad'
      ? {id, error:{message:'Refused'}}
      : {id, result:method === 'Runtime.evaluate' ? {result:{value:params.expression}} : {ok:true}}));
  }
  close() { this.dispatchEvent(new Event('close')); }
}

test('CDP resolves responses, reports errors, and releases disconnected requests', async () => {
  const original = globalThis.WebSocket;
  globalThis.WebSocket = Socket;
  try {
    const c = await connectCdp('ws://fixture', {timeout:100});
    assert.deepEqual(await c.send('Page.enable'), {ok:true});
    assert.equal(await c.run('answer'), 'answer');
    await assert.rejects(c.send('bad'), /Refused/);
    Socket.last.reply({method:'Runtime.exceptionThrown', params:{exceptionDetails:{text:'Page error'}}});
    assert.deepEqual(c.thrown, ['Page error']);
    const pending = c.send('hang');
    c.stop();
    await assert.rejects(pending, /closed/);
    await assert.rejects(c.send('Page.enable'), /closed/);
    const other = await connectCdp('ws://fixture', {timeout:10});
    await assert.rejects(other.send('hang'), /timed out/);
    other.stop();
  } finally { globalThis.WebSocket = original; }
});

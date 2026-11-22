const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const source = fs.readFileSync(path.join(__dirname, '../decoder-worker.js'), 'utf8').replace("import init, { DecoderEngine } from './wsjt-web.js';", '');
const flush = () => new Promise(setImmediate);
function transport() {
  const events = [], credits = [], forwarded = [];
  const main = { start() {}, postMessage(data) { credits.push(data); } };
  const recovery = { start() {}, postMessage(data) { forwarded.push(data); } };
  const engines = [];
  const context = { init: async () => {}, Float32Array, Map, Math, JSON, self: { postMessage(data) { events.push(data); } }, DecoderEngine: class {
    constructor() { engines.push(this); }
    set_role(role) { this.role = role; }
    set_priority_frequencies() {}
    push(offset, samples) { return JSON.stringify({ decodes: [], received_seconds: (offset+samples.length)/12000 }); }
    finish() { return JSON.stringify({decodes:[]}); }
    free() { this.freed = true; }
  } };
  vm.runInNewContext(source,context);
  return {events,credits,forwarded,engines,main,recovery,context};
}
test('slot and recovery share a credit only after both consume the packet; finish reaches both', async () => {
  const t=transport(); await t.context.self.onmessage({data:{role:'slot',rate:12000,epoch:0,port:t.main,recoveryPort:t.recovery}});
  t.main.onmessage({data:{kind:'samples',frame:0,samples:new Float32Array(3000)}});
  assert.equal(t.engines[0].role,'slot'); assert.equal(t.forwarded.length,1); assert.equal(t.credits.length,0);
  t.recovery.onmessage({data:{kind:'credit',frame:0}});
  assert.equal(t.credits.length,1); assert.equal(t.credits[0].frame,0);
  t.main.onmessage({data:{kind:'finish'}});
  assert.equal(t.forwarded.at(-1).kind,'finish'); assert.equal(t.events.at(-1).kind,'finished'); assert.equal(t.engines[0].freed,true);
});
test('a blocked recovery lane has a 60-second bound and never receives a premature credit', async () => {
  const t=transport(); await t.context.self.onmessage({data:{role:'slot',rate:12000,epoch:0,port:t.main,recoveryPort:t.recovery}});
  for(let i=0;i<240;i++)t.main.onmessage({data:{kind:'samples',frame:i*3000,samples:new Float32Array(3000)}});
  assert.equal(t.credits.length,0); assert.equal(t.forwarded.length,240);
  t.main.onmessage({data:{kind:'samples',frame:720000,samples:new Float32Array(3000)}});
  assert.equal(t.forwarded.length,240); assert.match(t.events.at(-1).error,/60-second/); assert.equal(t.engines[0].freed,true);
});
test('direct combined worker retains its existing credits and validates continuity', async () => {
  const t=transport(); await t.context.self.onmessage({data:{rate:12000,epoch:0,port:t.main}});
  t.main.onmessage({data:{kind:'samples',frame:0,samples:new Float32Array(3000)}});
  assert.equal(t.engines[0].role,'combined'); assert.equal(t.credits.length,1);
  t.main.onmessage({data:{kind:'samples',frame:4000,samples:new Float32Array(3000)}});
  assert.match(t.events.at(-1).error,/discontinuity/); await flush();
});

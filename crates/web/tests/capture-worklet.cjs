// Run with node --test crates/web/tests/capture-worklet.cjs (no npm dependencies).
const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const path = require('node:path');

function capture(rate = 12000) {
  let Processor;
  const events = [], chunks = [];
  const scope = vm.createContext({
    sampleRate: rate, currentFrame: 123456,
    AudioWorkletProcessor: class { constructor() { this.port = { postMessage: event => events.push(event) }; } },
    registerProcessor: (_, ctor) => { Processor = ctor; },
  });
  vm.runInContext(fs.readFileSync(path.join(__dirname, '../capture-worklet.js'), 'utf8'), scope);
  const processor = new Processor();
  const sink = { start() {}, postMessage: (event, transfer = []) => chunks.push(structuredClone(event, { transfer })) };
  processor.port.onmessage({ data: { kind: 'connect', port: sink } });
  return {
    events, chunks, sink,
    block(length = 128) {
      const input = new Float32Array(length).fill(0.25);
      processor.process([[input]]);
      scope.currentFrame += length;
    },
    skip() { scope.currentFrame += 128; },
    stop() { processor.port.onmessage({ data: { kind: 'stop' } }); },
  };
}

test('continuous samples and final partial chunk retain absolute audio frame positions', () => {
  const c = capture();
  for (let i = 0; i < 100; i++) c.block();
  c.stop();
  const samples = c.chunks.filter(e => e.kind === 'samples');
  assert.equal(samples.length, 5);
  assert.equal(samples[0].frame, 123456);
  assert.equal(samples[1].frame, 123456 + 3000);
  assert.equal(samples.reduce((n, e) => n + e.samples.length, 0), 12800);
  assert.ok(samples.every(e => e.samples.every(x => x === 0.25)));
  assert.equal(c.chunks.at(-1).kind, 'finish');
  c.stop();
  assert.equal(c.chunks.filter(e => e.kind === 'finish').length, 1);
});

test('overrun stops with an explicit error and never queues more than 60 seconds', () => {
  const c = capture();
  for (let i = 0; i < 62 * 12000 / 128; i++) c.block();
  assert.equal(c.chunks.filter(e => e.kind === 'samples').length, 240);
  assert.match(c.chunks.at(-1).error, /behind/);
  assert.match(c.events.at(-1).error, /behind/);
});

test('Stop with a full queue reports the unsent tail instead of silently losing it', () => {
  const c = capture();
  for (let i = 0; i < 60 * 12000 / 128; i++) c.block();
  c.block();
  c.stop();
  assert.equal(c.chunks.filter(e => e.kind === 'samples').length, 240);
  assert.match(c.chunks.at(-1).error, /final chunk/);
});

test('returned credits allow sustained capture without increasing the buffer limit', () => {
  const c = capture();
  let acknowledged = 0;
  for (let i = 0; i < 120 * 12000 / 128; i++) {
    c.block();
    const sent = c.chunks.filter(e => e.kind === 'samples').length;
    if (sent > acknowledged) { c.sink.onmessage({ data: { kind: 'credit' } }); acknowledged = sent; }
  }
  c.stop();
  assert.equal(c.chunks.filter(e => e.kind === 'samples').length, 480);
  assert.equal(c.chunks.at(-1).error, null);
});

test('audio clock gaps stop capture instead of joining discontinuous PCM', () => {
  const c = capture();
  c.block();
  c.skip();
  c.block();
  assert.match(c.chunks.at(-1).error, /discontinuity/);
  assert.equal(c.chunks.filter(e => e.kind === 'samples').length, 0);
});

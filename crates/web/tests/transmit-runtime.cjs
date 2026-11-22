const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const flush = () => new Promise(setImmediate);

test('saved station/output, selected sink, timed PTT, cancellation and device/error handling', async t => {
  t.mock.timers.enable({ apis: ['setTimeout', 'setInterval', 'Date'], now: 30000 });
  const listeners = new Map();
  global.document = { hidden: false, addEventListener: (name, fn) => listeners.set(name, fn), removeEventListener: (name, fn) => { if (listeners.get(name) === fn) listeners.delete(name); } };
  const storage = new Map();
  global.localStorage = { getItem: k => storage.get(k), setItem: (k,v) => storage.set(k,v) };
  let devices = [{ kind: 'audiooutput', deviceId: 'radio-card', label: 'USB Radio' }];
  let tracksStopped = 0;
  Object.defineProperty(global, 'navigator', { configurable: true, value: { mediaDevices: {
    enumerateDevices: async () => devices,
    getUserMedia: async () => ({ getTracks: () => [{ stop() { tracksStopped++; } }] }),
    addEventListener: (name, fn) => listeners.set(name, fn),
    removeEventListener: (name, fn) => { if (listeners.get(name) === fn) listeners.delete(name); },
  } } });
  let resumeWait = null, rejectSink = false, suspendOnSink = true;
  const contexts = [];
  global.AudioContext = class {
    constructor() { this.origin = Date.now(); this.state = 'running'; contexts.push(this); }
    get currentTime() { return (Date.now() - this.origin) / 1000; }
    resume() { this.resumes = (this.resumes || 0) + 1; this.state = 'running'; return resumeWait ? new Promise(resolve => { resumeWait = resolve; }) : Promise.resolve(); }
    async setSinkId(id) { if (rejectSink) throw new Error('Output permission denied'); this.sinkId = id; if (suspendOnSink) this.state = 'suspended'; }
    createGain() { return this.gainNode = { gain: { value: 1, cancelScheduledValues() {}, setTargetAtTime(value, time, constant) { this.value = value; this.timeConstant = constant; } }, connect(destination) { this.destination = destination; } }; }
    createBuffer(channels, length, rate) { assert.equal(channels, 1); assert.equal(length, 151680); assert.equal(rate, 12000); return { copyToChannel: data => { this.pcm = data; } }; }
    createBufferSource() { return this.source = { connect(destination) { this.destination = destination; }, disconnect() {}, stop() { this.stopped = true; }, start(time) { this.startTime = time; } }; }
    async close() { this.state = 'closed'; }
  };
  const code = fs.readFileSync(path.join(__dirname, '../src/transmit-runtime.js'), 'utf8');
  const api = await import(`data:text/javascript;base64,${Buffer.from(code).toString('base64')}`);
  assert.equal(api.saveTransmitPreferences('W9XYZ', 'EN37', 'radio-card', 'USB Radio'), '');
  assert.deepEqual(JSON.parse(api.loadTransmitPreferences()), { callsign: 'W9XYZ', grid: 'EN37', output_id: 'radio-card', output_label: 'USB Radio', attenuation_db: 0, auto_remove: true, cq_timeout_seconds: 180, sort_state: false, sort_signal: false, sort_distance: false, find_open_frequency: false });
  api.saveTransmitPreferences('W9XYZ', 'EN37', 'radio-card', 'USB Radio', 0, false, 60);
  assert.equal(JSON.parse(api.loadTransmitPreferences()).auto_remove, false);
  assert.equal(JSON.parse(api.loadTransmitPreferences()).cq_timeout_seconds, 60);
  assert.deepEqual(JSON.parse(await api.listAudioOutputs(true)), [{ id: 'radio-card', label: 'USB Radio' }]);
  api.saveTransmitPreferences('W9XYZ', 'EN37', 'radio-card', 'USB Radio', 0, false, 60, true, false, true);
  assert.deepEqual(['sort_state', 'sort_signal', 'sort_distance'].map(k => JSON.parse(api.loadTransmitPreferences())[k]), [true, false, true]);
  api.saveTransmitPreferences('W9XYZ', 'EN37', 'radio-card', 'USB Radio', 0, false, 60, true, false, true, true);
  assert.equal(JSON.parse(api.loadTransmitPreferences()).find_open_frequency, true);
  assert.equal(tracksStopped, 1);
  const events = [];
  let available = true, keyWait = null;
  const tx = api.createTransmitter(async () => { events.push('key'); if (keyWait) await new Promise(resolve => { keyWait = resolve; }); }, async () => { events.push('release'); }, () => available);
  api.setTransmitAttenuation(tx, 12);
  const pcm = new Float32Array(151680).fill(.25);
  const start = await api.playTransmission(tx, pcm, 2, 'radio-card', true);
  assert.equal(start, 45500);
  assert.equal(contexts[0].sinkId, 'radio-card');
  assert.equal(contexts[0].source.destination, contexts[0].gainNode);
  assert.ok(Math.abs(contexts[0].gainNode.gain.value - 10 ** (-12/20)) < 1e-12);
  api.setTransmitAttenuation(tx, 20);
  assert.ok(Math.abs(contexts[0].gainNode.gain.value - .1) < 1e-12);
  assert.equal(contexts[0].gainNode.gain.timeConstant, .015);
  api.setTransmitAttenuation(tx, -10); assert.equal(contexts[0].gainNode.gain.value, 1, 'no boost');
  api.setTransmitAttenuation(tx, 999); assert.equal(contexts[0].gainNode.gain.value, .001);
  api.setTransmitAttenuation(tx, 12);
  assert.equal(api.saveTransmitPreferences('W9XYZ', 'EN37', 'radio-card', 'USB Radio', 18), '');
  assert.equal(JSON.parse(api.loadTransmitPreferences()).attenuation_db, 18);
  assert.equal(contexts[0].resumes, 2, 'sink transition suspension is resumed after routing');
  suspendOnSink = false;
  assert.equal(events.length, 0, 'does not key throughout the waiting period');
  assert.equal(contexts[0].source.startTime, undefined, 'no audio before PTT acknowledgment');
  assert.equal(api.transmitterPlaying(tx), false, 'scheduled audio is not playing');
  pcm.fill(0); assert.equal(contexts[0].pcm[100], .25);
  await assert.rejects(api.playTransmission(tx, pcm, 2, 'radio-card', true), /already active/);
  t.mock.timers.tick(15000); await flush();
  assert.deepEqual(events, ['key']);
  assert.equal(contexts[0].source.startTime, 15.5);
  assert.equal(api.transmitterPlaying(tx), false, 'keyed ahead of scheduled audio');
  t.mock.timers.tick(500); await flush();
  assert.equal(api.transmitterPlaying(tx), true, 'audio clock reached frame start');
  t.mock.timers.tick(12640); await flush();
  assert.equal(api.transmitterPlaying(tx), false, 'audio ended before PTT release');
  contexts[0].source.onended(); t.mock.timers.tick(200); await flush();
  assert.deepEqual(events, ['key', 'release']);
  assert.equal(api.transmitterBusy(tx), false);
  assert.equal(contexts[0].state, 'running', 'armed queue keeps its user-primed output between messages');
  assert.equal(api.transmitterPrepared(tx), true);
  await api.playTransmission(tx, pcm, 2, 'radio-card', true);
  document.hidden = true; listeners.get('visibilitychange')(); await flush();
  assert.equal(api.transmitterBusy(tx), false);
  assert.equal(events.filter(e => e === 'key').length, 1, 'cancel before key never starts TX');
  document.hidden = false;
  keyWait = true;
  const next = await api.playTransmission(tx, pcm, 2, 'radio-card', true);
  t.mock.timers.tick(next - Date.now() - 500); await flush();
  const canceledContext = contexts.at(-1);
  const keys = events.filter(e => e === 'key').length;
  const stopping = api.stopTransmitter(tx);
  assert.equal(api.transmitterBusy(tx), true, 'remain busy until pending key and release finish');
  keyWait(); keyWait = null; await stopping; await flush();
  assert.equal(events.at(-1), 'release');
  assert.equal(events.filter(e => e === 'key').length, keys);
  assert.equal(canceledContext.source.startTime, undefined, 'late key ACK cannot start canceled audio');
  rejectSink = true;
  await assert.rejects(api.playTransmission(tx, pcm, 2, 'radio-card', true), /permission denied/);
  assert.equal(events.at(-1), 'release');
  rejectSink = false; devices = [];
  await assert.rejects(api.playTransmission(tx, pcm, 2, 'radio-card', true), /unavailable/);
  assert.equal(contexts.at(-1).sinkId, 'radio-card', 'missing saved device never falls back');
  devices = [{ kind: 'audiooutput', deviceId: 'radio-card', label: 'USB Radio' }];
  await api.playTransmission(tx, pcm, 2, 'radio-card', true);
  available = false; t.mock.timers.tick(100); await flush();
  assert.equal(api.transmitterBusy(tx), false);
  available = true;
  await api.playTransmission(tx, pcm, 2, 'radio-card', true);
  devices = []; listeners.get('devicechange')(); await flush();
  assert.equal(api.transmitterBusy(tx), false);
  assert.match(api.transmitterStatus(tx), /disconnected/);
  devices = [{ kind: 'audiooutput', deviceId: 'radio-card', label: 'USB Radio' }];
  keyWait = true;
  const late = await api.playTransmission(tx, pcm, 2, 'radio-card', true);
  t.mock.timers.tick(late - Date.now() - 500); await flush();
  t.mock.timers.tick(600); keyWait(); keyWait = null; await flush();
  assert.equal(contexts.at(-1).source.startTime, undefined, 'late key ACK skips transmission instead of playing off-slot');
  assert.equal(events.at(-1), 'release');
  assert.equal(api.transmitterBusy(tx), false);
  assert.match(api.transmitterStatus(tx), /not ready/);
  resumeWait = true;
  const pending = api.playTransmission(tx, pcm, 2, 'radio-card', true);
  const cancel = api.stopTransmitter(tx);
  resumeWait(); resumeWait = null; await cancel;
  await assert.rejects(pending, /canceled/);
  // Stop while natural completion is waiting for unkey must still close output.
  let releaseDone;
  const tx2 = api.createTransmitter(async () => {}, () => new Promise(resolve => { releaseDone = resolve; }), () => true);
  await api.playTransmission(tx2, pcm, 2, 'radio-card', true);
  const closingContext = contexts.at(-1);
  const natural = api.stopTransmitter(tx2, 'FT8 audio complete; receive requested.', true);
  await flush();
  const explicit = api.stopTransmitter(tx2);
  releaseDone(); await natural; await explicit;
  assert.equal(closingContext.state, 'closed');
  assert.equal(api.transmitterPrepared(tx2), false);
  // Failed unkey pauses the exchange instead of reporting a successful frame.
  const tx3 = api.createTransmitter(async () => {}, async () => { throw new Error('unkey failed'); }, () => true);
  await api.playTransmission(tx3, pcm, 2, 'radio-card', true);
  await api.stopTransmitter(tx3, 'FT8 audio complete; receive requested.', true);
  assert.equal(api.transmitterOutcome(tx3), 'error');
  assert.equal(api.transmitterPrepared(tx3), false);
  await api.disposeTransmitter(tx2); await api.disposeTransmitter(tx3);
  await api.disposeTransmitter(tx); assert.equal(listeners.size, 0);
  await assert.rejects(api.playTransmission(tx, pcm, 2, 'radio-card', true), /closed/);
});

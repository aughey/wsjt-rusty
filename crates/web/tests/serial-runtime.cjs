const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const source = fs.readFileSync(path.join(__dirname, '../src/serial-runtime.js'), 'utf8');
const runtime = import(`data:text/javascript;base64,${Buffer.from(source).toString('base64')}`);

function fakePort() {
  const writes = [], signals = [];
  let input;
  return {
    writes, signals, closed: false,
    async open(options) {
      this.options = options;
      this.readable = new ReadableStream({ start(c) { input = c; } });
      this.writable = new WritableStream({ write(bytes) { writes.push([...bytes]); } });
    },
    async setSignals(value) { signals.push(value); },
    async close() { this.closed = true; this.readable = null; this.writable = null; },
    send(bytes) { input.enqueue(new Uint8Array(bytes)); },
  };
}
function setup(port, request = async () => port) {
  Object.defineProperty(globalThis, 'navigator', { configurable: true, value: { serial: { requestPort: request } } });
  globalThis.isSecureContext = true;
}

test('Web Serial lifecycle, exact bytes, same-port PTT and idempotent release', async () => {
  const api = await runtime;
  const port = fakePort(); setup(port);
  const s = api.createSerial();
  await api.openSerial(s, 19200, 'requestToSend');
  assert.equal(port.options.flowControl, 'none');
  assert.deepEqual(port.signals, [{ requestToSend: false }]);
  await api.writeSerial(s, new Uint8Array([0xfe, 0xfe, 0x5e, 0xe0, 3, 0xfd]));
  port.send([0xfe, 0xfe, 0xe0]);
  assert.deepEqual([...await api.readSerial(s, 1000)], [0xfe, 0xfe, 0xe0]);
  api.armRelease(s, []);
  await api.setPttLine(s, 'requestToSend', true);
  const closing = api.closeSerial(s);
  assert.equal(api.closeSerial(s), closing);
  await closing;
  assert.deepEqual(port.signals.at(-1), { requestToSend: false });
  assert.equal(port.closed, true);
  assert.equal(api.isSerialOpen(s), false);
});

test('cancel during permission and during open releases late ports', async () => {
  const api = await runtime;
  let grant;
  const port = fakePort(); setup(port, () => new Promise(resolve => { grant = resolve; }));
  const s = api.createSerial();
  const opening = api.openSerial(s, 9600);
  await api.closeSerial(s); grant(port);
  await assert.rejects(opening, /cancelled/);
  assert.equal(port.options, undefined);

  let finishOpen;
  const late = fakePort(); setup(late);
  const actual = late.open.bind(late);
  late.open = async options => { await actual(options); await new Promise(resolve => { finishOpen = resolve; }); };
  const s2 = api.createSerial();
  const opening2 = api.openSerial(s2, 9600);
  await new Promise(resolve => setImmediate(resolve));
  await api.closeSerial(s2); finishOpen();
  await assert.rejects(opening2, /cancelled/);
  assert.equal(late.closed, true);
});

test('read timeout closes transport and sends modern CAT release', async () => {
  const api = await runtime;
  const port = fakePort(); setup(port);
  const s = api.createSerial(); await api.openSerial(s, 19200);
  api.armRelease(s, new Uint8Array([0xfe,0xfe,0x5e,0xe0,0x1c,1,0,0xfd]));
  await assert.rejects(api.readSerial(s, 5), /timeout/);
  await api.closeSerial(s);
  assert.deepEqual(port.writes.at(-1), [0xfe,0xfe,0x5e,0xe0,0x1c,1,0,0xfd]);
  assert.equal(port.closed, true);
});

test('TX watchdog closes and releases the selected line', async () => {
  const api = await runtime;
  const port = fakePort(); setup(port);
  const s = api.createSerial(); await api.openSerial(s, 9600);
  await api.setPttLine(s, 'dataTerminalReady', true);
  const original = globalThis.setTimeout;
  let watchdog;
  globalThis.setTimeout = (callback, ms, ...rest) => {
    if (ms === 30000) { watchdog = callback; return original(() => {}, 100000); }
    return original(callback, ms, ...rest);
  };
  try { api.armRelease(s, []); } finally { globalThis.setTimeout = original; }
  watchdog(); await api.closeSerial(s);
  assert.deepEqual(port.signals.at(-1), { dataTerminalReady: false });
  assert.equal(port.closed, true);
});

test('unsupported context fails explicitly', async () => {
  const api = await runtime;
  setup(fakePort()); globalThis.isSecureContext = false;
  assert.equal(api.serialSupported(), false);
  await assert.rejects(api.openSerial(api.createSerial(), 19200), /localhost or HTTPS/);
});


test('radio preferences use separate persistent storage and report unavailable storage', async () => {
  const api = await runtime;
  const values = new Map([['wsjt-rusty.transmit.v1', 'station-settings']]);
  globalThis.localStorage = { getItem: key => values.get(key), setItem: (key, value) => values.set(key, value) };
  assert.equal(api.loadRadioPreferences(), '{}');
  const json = JSON.stringify({ baud: 9600, address: '5E', frequency: '7074000', mode: 1, power: '50', method: 'cat-rts' });
  assert.equal(api.saveRadioPreferences(json), '');
  assert.equal(api.loadRadioPreferences(), json);
  assert.equal(values.get('wsjt-rusty.transmit.v1'), 'station-settings');
  globalThis.localStorage = { getItem() { throw new Error('blocked'); }, setItem() { throw new Error('blocked'); } };
  assert.equal(api.loadRadioPreferences(), '{}');
  assert.match(api.saveRadioPreferences(json), /cannot be saved/);
});

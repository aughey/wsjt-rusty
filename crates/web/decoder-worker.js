// Transport only. Separate slot/recovery WASM instances keep full recovery off
// the deadline-sensitive lane; all signal processing remains Rust.
import init, { DecoderEngine } from './wsjt-web.js';
const ready = init();
let engine, firstFrame, expectedFrame, port, epoch, rate;
let role = 'combined';
let finished = false;
let priorities = new Float32Array();
let recoveryPort;
const outstanding = new Map();
let outstandingSamples = 0;

function fail(error) {
  finished = true;
  engine?.free(); engine = undefined;
  self.postMessage({ kind: 'error', error: String(error) });
}
function credit(frame, lane) {
  const packet = outstanding.get(frame);
  if (!packet) throw new Error('Unexpected decoder credit');
  packet[lane] = true;
  if (packet.slot && packet.recovery) {
    outstandingSamples -= packet.samples;
    outstanding.delete(frame);
    port.postMessage({ kind: 'credit', frame });
  }
}
function receive({ data }) {
  if (finished) return;
  try {
    if (data.kind === 'samples') {
      if (!engine) {
        firstFrame = expectedFrame = data.frame;
        engine = new DecoderEngine(rate, epoch + firstFrame / rate * 1000);
        engine.set_role(role);
        engine.set_priority_frequencies(priorities);
      }
      if (data.frame !== expectedFrame) throw new Error('Audio discontinuity; capture stopped.');
      if (recoveryPort) {
        // Worklet credits return only once BOTH lanes have consumed a packet.
        // Thus the existing 60-second aggregate in-flight bound still applies.
        if (outstandingSamples + data.samples.length > rate * 60 || outstanding.size >= Math.ceil(60 * 4)) throw new Error('Decoder transport exceeded its 60-second bound');
        outstanding.set(data.frame, { slot: false, recovery: false, samples: data.samples.length });
        outstandingSamples += data.samples.length;
        recoveryPort.postMessage(data); // clone before WASM uses the original PCM
      }
      const update = JSON.parse(engine.push(data.frame - firstFrame, data.samples));
      expectedFrame += data.samples.length;
      self.postMessage({ kind: 'update', update });
      if (recoveryPort) credit(data.frame, 'slot');
      else port.postMessage({ kind: 'credit', frame: data.frame });
    } else if (data.kind === 'finish') {
      recoveryPort?.postMessage(data);
      const update = engine ? JSON.parse(engine.finish()) : null;
      finished = true;
      engine?.free(); engine = undefined;
      self.postMessage({ kind: 'finished', update, error: data.error || null });
    }
  } catch (error) { fail(error); }
}
self.onmessage = async ({ data }) => {
  try {
    await ready;
    if (data.kind === 'priorities') {
      priorities = new Float32Array(data.frequencies);
      engine?.set_priority_frequencies(priorities);
      self.postMessage({ kind: 'priorities-applied', frequencies: Array.from(priorities) });
      return;
    }
    role = data.role || 'combined';
    rate = data.rate; epoch = data.epoch; port = data.port;
    recoveryPort = data.recoveryPort;
    if (recoveryPort) {
      recoveryPort.onmessage = ({ data }) => {
        if (finished) return;
        try { credit(data.frame, 'recovery'); } catch (error) { fail(error); }
      };
      recoveryPort.start();
    }
    port.onmessage = receive; port.start();
    self.postMessage({ kind: 'ready' });
  } catch (error) { fail(error); }
};

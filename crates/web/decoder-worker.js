// Transport only: resampling, windowing and FT8 decoding run in Rust/WASM.
import init, { DecoderEngine } from './wsjt-web.js';
const ready = init();
let engine;
let firstFrame;
let expectedFrame;
let port;
let epoch;
let rate;
let finished = false;

function fail(error) {
  finished = true;
  engine?.free();
  engine = undefined;
  self.postMessage({ kind: 'error', error: String(error) });
}

function receive({ data }) {
  if (finished) return;
  try {
    if (data.kind === 'samples') {
      if (!engine) {
        firstFrame = expectedFrame = data.frame;
        engine = new DecoderEngine(rate, epoch + firstFrame / rate * 1000);
      }
      if (data.frame !== expectedFrame) throw new Error('Audio discontinuity; capture stopped.');
      const update = JSON.parse(engine.push(data.frame - firstFrame, data.samples));
      expectedFrame += data.samples.length;
      self.postMessage({ kind: 'update', update });
      // Credit is returned only after synchronous processing completes.
      port.postMessage({ kind: 'credit' });
    } else if (data.kind === 'finish') {
      const update = engine ? JSON.parse(engine.finish()) : null;
      finished = true;
      engine?.free();
      engine = undefined;
      self.postMessage({ kind: 'finished', update, error: data.error || null });
    }
  } catch (error) {
    fail(error);
  }
}

self.onmessage = async ({ data }) => {
  try {
    await ready;
    rate = data.rate;
    epoch = data.epoch;
    port = data.port;
    port.onmessage = receive;
    port.start();
    self.postMessage({ kind: 'ready' });
  } catch (error) { fail(error); }
};

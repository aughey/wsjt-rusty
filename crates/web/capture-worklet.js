// Copies continuous mono PCM. No FFT, filtering, resampling or decoding here.
class CaptureProcessor extends AudioWorkletProcessor {
  constructor() {
    super();
    this.active = false;
    this.used = 0;
    this.pending = 0;
    this.peak = 0;
    this.buffer = new Float32Array(Math.ceil(sampleRate / 4)); // quarter-second transport chunks
    this.port.onmessage = ({ data }) => {
      if (data.kind === 'connect') {
        this.sink = data.port;
        this.sink.onmessage = () => { this.pending--; };
        this.sink.start();
        this.active = true;
      } else if (data.kind === 'stop') {
        this.finish();
      }
    };
  }
  send() {
    // At most 60 seconds of PCM in transit, including the chunk being decoded.
    if (this.pending >= Math.floor(60 * sampleRate / this.buffer.length)) {
      this.finish('FT8 decoder fell more than 60 seconds behind. Capture stopped; no audio gap was hidden.');
      return false;
    }
    const samples = this.used === this.buffer.length ? this.buffer : this.buffer.slice(0, this.used);
    // Meter every captured sample, independently of display/decoder scheduling.
    this.port.postMessage({ kind: 'input-level', peak: this.peak });
    this.peak = 0;
    this.sink.postMessage({ kind: 'samples', frame: this.frame, samples }, [samples.buffer]);
    this.pending++;
    this.buffer = new Float32Array(Math.ceil(sampleRate / 4));
    this.used = 0;
    return true;
  }
  finish(error = null) {
    if (!this.active) return;
    if (!error && this.used && this.pending >= Math.floor(60 * sampleRate / this.buffer.length)) {
      error = 'FT8 decoder fell more than 60 seconds behind. Capture stopped before its final chunk.';
    }
    // On overflow, retain only the contiguous audio already delivered.
    if (!error && this.used) this.send();
    this.active = false;
    this.sink.postMessage({ kind: 'finish', error });
    this.port.postMessage({ kind: 'stopped', error });
  }
  process(inputs) {
    if (!this.active) return true;
    const mono = inputs[0]?.[0];
    if (!mono?.length) {
      // Do not manufacture silence if a previously active source disappears.
      if (this.expected !== undefined) this.finish('Audio input disappeared.');
      return true;
    }
    if (this.expected !== undefined && currentFrame !== this.expected) {
      this.finish('Audio clock discontinuity. Capture stopped.');
      return true;
    }
    this.expected = currentFrame + mono.length;
    for (let i = 0; i < mono.length; i++) {
      if (this.used === 0) this.frame = currentFrame + i;
      this.buffer[this.used++] = mono[i];
      this.peak = Math.max(this.peak, Math.abs(mono[i]));
      if (this.used === this.buffer.length && !this.send()) break;
    }
    return true;
  }
}
registerProcessor('wsjt-capture', CaptureProcessor);

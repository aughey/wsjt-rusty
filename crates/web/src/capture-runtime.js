// Browser transport and resource lifecycle. All receive DSP lives in Rust.
export function createCancellation() { return { aborted: false, abort: null }; }
export function cancelCapture(token) { token.aborted = true; token.abort?.(); }

export async function openCapture(onEvent, token) {
  const context = new AudioContext();
  let stream, source, node, worker, recoveryWorker;
  let closed = false;
  let stopping = false;
  let resolveDone;
  let cancelReady;
  const done = new Promise(resolve => { resolveDone = resolve; });
  const analyser = context.createAnalyser();
  // Time-domain snapshots for RustFFT visualization, separate from continuous PCM.
  analyser.fftSize = 16384;
  const releaseInput = () => {
    stream?.getTracks().forEach(track => track.stop());
    source?.disconnect();
    node?.disconnect();
    if (context.state !== 'closed') context.close().catch(() => {});
  };
  const abort = () => {
    if (closed) return;
    closed = true;
    cancelReady?.(new Error('Capture cancelled.'));
    releaseInput();
    worker?.terminate();
    recoveryWorker?.terminate();
    resolveDone();
  };
  token.abort = abort;
  const checkCancelled = () => {
    if (token.aborted || closed) {
      // getUserMedia itself cannot be cancelled. Release a late permission grant.
      stream?.getTracks().forEach(track => track.stop());
      throw new Error('Capture cancelled.');
    }
  };
  const emit = event => { if (!closed) onEvent(JSON.stringify(event)); };
  const stop = () => {
    if (!closed && !stopping) {
      stopping = true;
      // Worklet flushes its short tail before the input graph is released.
      node.port.postMessage({ kind: 'stop' });
    }
    return done;
  };
  try {
    checkCancelled();
    await context.resume();
    checkCancelled();
    stream = await navigator.mediaDevices.getUserMedia({
      audio: { channelCount: 1, echoCancellation: false, noiseSuppression: false, autoGainControl: false },
      video: false,
    });
    checkCancelled();
    await context.audioWorklet.addModule(new URL('capture-worklet.js', document.baseURI));
    checkCancelled();
    source = context.createMediaStreamSource(stream);
    source.connect(analyser);
    node = new AudioWorkletNode(context, 'wsjt-capture', { channelCount: 1, channelCountMode: 'explicit', outputChannelCount: [1] });
    recoveryWorker = new Worker(new URL('decoder-worker.js', document.baseURI), { type: 'module' });
    worker = new Worker(new URL('decoder-worker.js', document.baseURI), { type: 'module' });
    const channel = new MessageChannel();
    const recoveryChannel = new MessageChannel();
    const epoch = Date.now() - context.currentTime * 1000;
    let readyResolve, readyReject;
    let slotReady = false, recoveryReady = false;
    let slotFinished = null, recoveryFinished = null;
    const ready = new Promise((resolve, reject) => { readyResolve = resolve; readyReject = reject; });
    cancelReady = readyReject;
    const timeout = setTimeout(() => readyReject(new Error('Decoder worker startup timed out.')), 30000);
    const workerError = event => {
      const error = event.message || 'Decoder worker failed.';
      readyReject(new Error(error)); emit({ kind: 'error', error }); abort();
    };
    worker.onerror = recoveryWorker.onerror = workerError;
    function finishIfReady() {
      if (!slotFinished || !recoveryFinished) return;
      const update = slotFinished.update;
      if (update && recoveryFinished.update) update.decodes.push(...recoveryFinished.update.decodes);
      emit({ kind: 'finished', update, error: slotFinished.error || recoveryFinished.error || null });
      abort();
    }
    function message(data, recovery) {
      if (closed) return;
      if (data.kind === 'ready') {
        if (recovery) recoveryReady = true; else slotReady = true;
        if (slotReady && recoveryReady) readyResolve();
        return;
      }
      if (data.kind === 'error') { readyReject(new Error(data.error)); emit(data); abort(); return; }
      if (data.kind === 'finished') {
        if (recovery) recoveryFinished = data; else slotFinished = data;
        finishIfReady(); return;
      }
      if (recovery && data.kind === 'update') {
        emit({ kind: 'recovery-results', decodes: data.update.decodes, recovery_end_ms: data.update.capture_end_ms });
      } else { emit(data); }
    }
    worker.onmessage = ({data}) => message(data, false);
    recoveryWorker.onmessage = ({data}) => message(data, true);
    recoveryWorker.postMessage({ role: 'recovery', rate: context.sampleRate, epoch, port: recoveryChannel.port1 }, [recoveryChannel.port1]);
    worker.postMessage({ role: 'slot', rate: context.sampleRate, epoch, port: channel.port1, recoveryPort: recoveryChannel.port2 }, [channel.port1, recoveryChannel.port2]);
    try { await ready; } finally { clearTimeout(timeout); }
    checkCancelled();
    node.port.onmessage = ({ data }) => {
      if (data.kind === 'input-level') emit(data);
      if (data.kind === 'stopped') {
        stopping = true;
        emit({ kind: 'flushing', error: data.error });
        releaseInput();
      }
    };
    node.onprocessorerror = () => {
      emit({ kind: 'error', error: 'Audio capture processor failed; capture stopped.' });
      abort();
    };
    stream.getAudioTracks().forEach(track => track.addEventListener('ended', () => {
      if (!stopping && !closed) {
        emit({ kind: 'flushing', error: 'Audio device disconnected. Finishing captured audio.' });
        stop();
      }
    }));
    context.onstatechange = () => {
      if (!stopping && !closed && context.state !== 'running') {
        emit({ kind: 'flushing', error: 'Audio clock suspended. Capture stopped to preserve timestamps.' });
        // A suspended context may not process another quantum, but its port can
        // still flush the PCM already copied by the processor.
        stop();
      }
    };
    node.port.postMessage({ kind: 'connect', port: channel.port2 }, [channel.port2]);
    source.connect(node);
    // The processor outputs silence. Connecting it keeps capture rendering live.
    node.connect(context.destination);
    return { setPriorities: frequencies => { const message = { kind: 'priorities', frequencies: Array.from(frequencies) }; worker.postMessage(message); recoveryWorker.postMessage(message); }, analyser, sampleRate: context.sampleRate, audioTimeMs: () => epoch + context.currentTime * 1000, stop, abort };
  } catch (error) {
    abort();
    throw error;
  }
}

export function captureAnalyser(capture) { return capture.analyser; }
export function captureRate(capture) { return capture.sampleRate; }
export function captureTime(capture) { return capture.audioTimeMs(); }
export function stopCapture(capture) { return capture.stop(); }
export function abortCapture(capture) { capture.abort(); }

export function captureSetPriorities(capture, frequencies) { capture.setPriorities(frequencies); }

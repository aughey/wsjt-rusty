// Device/storage/playback transport. Packing, DSP and radio commands are Rust.
const preferencesKey = 'wsjt-rusty.transmit.v1';
export function loadTransmitPreferences() {
  try { return localStorage.getItem(preferencesKey) || '{}'; } catch (_) { return '{}'; }
}
export function saveTransmitPreferences(callsign, grid, output_id, output_label, attenuation_db = 0, auto_remove = true, cq_timeout_seconds = 180, sort_state = false, sort_signal = false, sort_distance = false, find_open_frequency = false) {
  try { localStorage.setItem(preferencesKey, JSON.stringify({ callsign, grid, output_id, output_label, attenuation_db, auto_remove, cq_timeout_seconds, sort_state, sort_signal, sort_distance, find_open_frequency })); return ''; }
  catch (_) { return 'Browser storage is unavailable; these settings cannot be saved.'; }
}
export function outputSelectionSupported() { return typeof AudioContext.prototype.setSinkId === 'function'; }
export async function listAudioOutputs(requestPermission) {
  if (!outputSelectionSupported()) throw new Error('This browser cannot select an audio output. Use a browser with AudioContext.setSinkId support.');
  if (requestPermission) {
    if (navigator.mediaDevices.selectAudioOutput) {
      await navigator.mediaDevices.selectAudioOutput();
    } else {
      const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
      stream.getTracks().forEach(track => track.stop());
    }
  }
  const devices = await navigator.mediaDevices.enumerateDevices();
  return JSON.stringify(devices.filter(d => d.kind === 'audiooutput').map(d => ({ id: d.deviceId, label: d.label || 'Audio output' })));
}
export function createTransmitter(key, release, available, validate = () => true) {
  const tx = { context: null, source: null, gainNode: null, attenuationDb: 0, generation: 0, busy: false, disposed: false,
    key, release, available, validate, keyPromise: null, stopping: null, preparing: false, outcome: '', lastStatus: '', useRadio: false };
  tx.visibility = () => { if (document.hidden) void stopTransmitter(tx, 'Tab hidden; contacting paused.'); };
  tx.deviceChange = () => {
    if (!tx.busy) return;
    void navigator.mediaDevices.enumerateDevices().then(devices => {
      if (tx.busy && tx.outputId !== 'default' && !devices.some(d => d.kind === 'audiooutput' && d.deviceId === tx.outputId)) {
        void stopTransmitter(tx, 'Selected audio output disconnected; transmission canceled.');
      }
    }).catch(() => { void stopTransmitter(tx, 'Could not verify audio output; transmission canceled.'); });
  };
  document.addEventListener('visibilitychange', tx.visibility);
  navigator.mediaDevices?.addEventListener?.('devicechange', tx.deviceChange);
  return tx;
}
// Device-output level only: never amplify above the existing generated PCM.
export function setTransmitAttenuation(tx, db) {
  tx.attenuationDb = Number.isFinite(db) ? Math.min(60, Math.max(0, db)) : 60;
  if (tx.gainNode && tx.context) {
    const now = tx.context.currentTime;
    tx.gainNode.gain.cancelScheduledValues(now);
    tx.gainNode.gain.setTargetAtTime(10 ** (-tx.attenuationDb / 20), now, 0.015);
  }
}
export function stopTransmitter(tx, reason = 'Audio canceled; receive requested.', keepOutput = false) {
  if (tx.stopping) {
    if (!keepOutput) { tx.closeRequested = true; tx.lastStatus = reason; tx.outcome = 'canceled'; }
    return tx.stopping;
  }
  tx.closeRequested = !keepOutput;
  tx.generation++;
  clearTimeout(tx.keyTimer); clearTimeout(tx.endTimer); clearInterval(tx.monitor);
  if (tx.source) {
    tx.source.onended = null;
    try { tx.source.stop(); } catch (_) { /* not started or already ended */ }
    tx.source.disconnect(); tx.source = null;
  }
  const context = tx.context;
  if (!keepOutput) tx.context = null;
  tx.outcome = keepOutput && reason.startsWith('FT8 audio complete') ? 'complete' : 'canceled';
  tx.lastStatus = reason;
  const keyPromise = tx.keyPromise;
  const useRadio = tx.useRadio;
  tx.stopping = (async () => {
    await Promise.resolve(); // Publish stopping state before any synchronous completion.
    try {
      // Serialize release after any pending key request, including a lost ACK.
      if (keyPromise) await keyPromise.catch(() => {});
      if (useRadio) await tx.release();
    } catch (error) { tx.lastStatus = `Radio release failed: ${error.message || error}. Verify RX at radio.`; tx.outcome = 'error'; tx.closeRequested = true; }
    finally {
      if (context && tx.closeRequested) { if (tx.context === context) tx.context = null; tx.gainNode = null; await context.close().catch(() => {}); }
      tx.keyPromise = null; tx.useRadio = false; tx.busy = false; tx.stopping = null;
    }
  })();
  return tx.stopping;
}
export async function disposeTransmitter(tx) {
  tx.disposed = true;
  document.removeEventListener('visibilitychange', tx.visibility);
  navigator.mediaDevices?.removeEventListener?.('devicechange', tx.deviceChange);
  await stopTransmitter(tx);
}
export function transmitterBusy(tx) { return tx.busy || tx.preparing || !!tx.stopping; }
export function transmitterPlaying(tx) {
  return !!tx.source && tx.sourceStarted && tx.context?.state === 'running'
    && tx.context.currentTime >= tx.startTime && tx.context.currentTime < tx.startTime + 12.64;
}
export function transmitterOutcome(tx) { return tx.outcome; }
export function transmitterPrepared(tx) { return !!tx.context && tx.context.state === 'running' && !tx.preparing; }
export function cancelQueuedTransmission(tx) { return stopTransmitter(tx, 'Current attempt canceled; receive requested.', true); }

async function boundedAudio(promise) {
  let timer;
  try { return await Promise.race([promise, new Promise((_, reject) => { timer = setTimeout(() => reject(new Error('Audio output is blocked or unavailable. Click Start contacting again to resume it.')), 3000); })]); }
  finally { clearTimeout(timer); }
}
export async function prepareTransmitter(tx, outputId) {
  if (tx.disposed || tx.busy || tx.preparing || tx.stopping) throw new Error('Audio output is already active or closed');
  if (!outputId) throw new Error('Select your radio audio output first');
  if (!outputSelectionSupported()) throw new Error('Audio output selection is unavailable in this browser');
  if (document.hidden) throw new Error('Keep this tab visible for contacting');
  tx.preparing = true;
  const generation = tx.generation;
  const current = () => generation === tx.generation && !tx.disposed;
  try {
    let context = tx.context;
    if (!context || context.state === 'closed') context = new AudioContext();
    if (tx.context !== context || !tx.gainNode) {
      tx.gainNode = context.createGain();
      tx.gainNode.gain.value = 10 ** (-tx.attenuationDb / 20);
      tx.gainNode.connect(context.destination);
    }
    tx.context = context; tx.outputId = outputId;
    // Unlock from the click gesture, then finish the sink transition before the
    // final resume. Some drivers suspend the context while changing the sink.
    context.onstatechange = null; context.onsinkchange = null;
    const unlocked = context.resume();
    unlocked.catch(() => {});
    await boundedAudio(context.setSinkId(outputId === 'default' ? '' : outputId));
    await boundedAudio(unlocked);
    if (!current()) throw new Error('Transmission canceled');
    if (context.state !== 'running') await boundedAudio(context.resume());
    if (!current()) throw new Error('Transmission canceled');
    if (context.state !== 'running') throw new Error(`Audio output remains ${context.state}; click Start contacting to resume after checking the radio device`);
    const devices = await navigator.mediaDevices.enumerateDevices();
    if (!current()) throw new Error('Transmission canceled');
    if (outputId !== 'default' && !devices.some(d => d.kind === 'audiooutput' && d.deviceId === outputId)) throw new Error('Saved audio output is unavailable; select the radio card again');
    context.onstatechange = () => { if (tx.context === context && context.state !== 'running') void stopTransmitter(tx, 'Audio output interrupted; contacting paused.'); };
    context.onsinkchange = () => { if (tx.context === context && context.sinkId !== (outputId === 'default' ? '' : outputId)) void stopTransmitter(tx, 'Audio output changed; contacting paused.'); };
  } catch (error) {
    if (current()) await stopTransmitter(tx, `Audio preparation failed: ${error.message || error}`);
    throw error;
  } finally { tx.preparing = false; }
}
export function transmitterStatus(tx) { return tx.lastStatus; }
export function transmitterPhase(tx) {
  if (tx.stopping) return 'Releasing radio and audio output…';
  if (!tx.busy) return '';
  if (!tx.source || !tx.context) return 'Opening selected audio output…';
  const remaining = tx.startTime - tx.context.currentTime;
  return remaining > 0 ? `Waiting · starts in ${remaining.toFixed(1)} s`
    : `Playing FT8 audio · ${Math.max(0, 12.64 + remaining).toFixed(1)} s remaining`;
}
export async function playTransmission(tx, samples, receiveSlot, outputId, useRadio, requestedStartMs = NaN, planId = NaN) {
  if (tx.disposed || tx.busy || tx.preparing || tx.stopping) throw new Error('Audio output is already active or closed');
  // The caller has reserved PTT before entering this function. Even setup failures release it.
  tx.useRadio = useRadio; tx.busy = true;
  const generation = ++tx.generation;
  const current = () => generation === tx.generation && !tx.disposed;
  const check = () => { if (!current()) throw new Error('Transmission canceled'); };
  const approved = () => tx.validate(planId);
  const supersede = async () => {
    await stopTransmitter(tx, 'Reply superseded by received evidence; waiting for a current reply.', true);
    if (tx.outcome !== 'error') tx.outcome = 'superseded';
  };
  try {
    if (document.hidden) throw new Error('Keep this tab visible for timed audio');
    if (!outputId) throw new Error('Select your radio audio output first');
    if (!(samples instanceof Float32Array) || samples.length !== 151680 || !samples.every(Number.isFinite)) throw new Error('Invalid FT8 PCM');
    const pcm = samples.slice(); // Own samples before awaiting; WASM memory may grow.
    if (!outputSelectionSupported()) throw new Error('Audio output selection is unavailable in this browser');
    if (!transmitterPrepared(tx) || tx.outputId !== outputId) {
      // Compatibility for direct callers. The queue prepares once in its Start click.
      tx.busy = false;
      await prepareTransmitter(tx, outputId); check();
      tx.busy = true;
    }
    const context = tx.context;
    if (useRadio && !tx.available()) throw new Error('Radio disconnected before transmit');
    const devices = await navigator.mediaDevices.enumerateDevices(); check();
    if (outputId !== 'default' && !devices.some(d => d.kind === 'audiooutput' && d.deviceId === outputId)) throw new Error('Saved audio output is unavailable; select the radio card again');
    if (!approved()) { await supersede(); return requestedStartMs; }
    const audioNow = context.currentTime;
    const now = Date.now();
    let slot = Math.ceil((now + 1000 - 500) / 15000);
    if (((slot % 2) + 2) % 2 === ((receiveSlot % 2) + 2) % 2) slot++;
    const startMs = Number.isFinite(requestedStartMs) ? requestedStartMs : slot * 15000 + 500;
    if (startMs - now < 150 || startMs - now > 31000) throw new Error('CQ response deadline missed; wait for a fresh CQ');
    const buffer = context.createBuffer(1, pcm.length, 12000);
    buffer.copyToChannel(pcm, 0);
    const source = context.createBufferSource();
    source.buffer = buffer; source.connect(tx.gainNode); tx.source = source; tx.sourceStarted = false;
    tx.startTime = audioNow + (startMs - now) / 1000;
    tx.outcome = '';
    source.onended = () => {
      if (current()) tx.endTimer = setTimeout(() => { void stopTransmitter(tx, 'FT8 audio complete; receive requested.', true); }, 200);
    };
    tx.monitor = setInterval(() => {
      if (!current()) return;
      if (Date.now() < startMs && !approved()) { void supersede(); return; }
      if (useRadio && !tx.available()) void stopTransmitter(tx, 'Radio disconnected; transmission canceled.');
    }, 100);
    if (useRadio) {
      tx.keyTimer = setTimeout(async () => {
        if (!current()) return;
        try {
          if (Date.now() > startMs - 50) throw new Error('Missed keying deadline; transmission canceled');
          if (!approved()) { await supersede(); return; }
          tx.keyPromise = Promise.resolve(tx.key());
          await tx.keyPromise; check();
          if (!approved()) { await supersede(); return; }
          if (context.currentTime >= tx.startTime - .025 || Date.now() > startMs - 25) throw new Error('PTT was not ready before UTC start; transmission canceled');
          source.start(tx.startTime);
          tx.sourceStarted = true;
        } catch (error) { if (current()) { await stopTransmitter(tx, error.message || String(error)); tx.outcome = 'error'; } }
      }, Math.max(0, startMs - Date.now() - 500));
    } else {
      if (!approved()) { await supersede(); return startMs; }
      source.start(tx.startTime); tx.sourceStarted = true;
    }
    return startMs;
  } catch (error) {
    if (current()) { await stopTransmitter(tx, `Transmission failed: ${error.message || error}`); tx.outcome = 'error'; }
    throw error;
  }
}

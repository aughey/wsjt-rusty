const radioPreferencesKey = 'wsjt-rusty.radio.v1';
export function loadRadioPreferences() {
  try { return localStorage.getItem(radioPreferencesKey) || '{}'; } catch (_) { return '{}'; }
}
export function saveRadioPreferences(json) {
  try { localStorage.setItem(radioPreferencesKey, json); return ''; }
  catch (_) { return 'Browser storage is unavailable; radio settings cannot be saved.'; }
}
// Web Serial transport only. CI-V commands, BCD and reply matching live in Rust.
export function serialSupported() {
  return globalThis.isSecureContext && !!globalThis.navigator?.serial;
}
export function createSerial() {
  const s = { closed: false, port: null, reader: null, writer: null, timer: null,
    off: null, line: null, closing: null };
  s.onExit = () => { void closeSerial(s).catch(() => {}); };
  globalThis.addEventListener?.("pagehide", s.onExit);
  return s;
}
export async function openSerial(s, baud, line = "") {
  if (!serialSupported()) throw new Error('Web Serial requires a supported browser on localhost or HTTPS');
  const port = await navigator.serial.requestPort(); // called from the click gesture
  if (s.closed) throw new Error('Connection cancelled');
  s.port = port;
  try {
    await port.open({ baudRate: baud, dataBits: 8, stopBits: 1, parity: 'none', flowControl: 'none' });
    if (s.closed) { await port.close(); throw new Error('Connection cancelled'); }
    if (line) { s.line = line; await port.setSignals({ [line]: false }); }
    if (s.closed) { await port.close(); throw new Error("Connection cancelled"); }
    s.onDisconnect = event => { if (event.target === port || event.port === port) s.onExit(); };
    navigator.serial.addEventListener?.("disconnect", s.onDisconnect);
    s.reader = port.readable.getReader();
    s.writer = port.writable.getWriter();
  } catch (error) {
    await closeSerial(s).catch(() => {});
    throw error;
  }
}
async function bounded(s, promise, milliseconds) {
  let timer;
  try {
    return await Promise.race([promise, new Promise((_, reject) => {
      timer = setTimeout(() => reject(new Error('Serial timeout; check cable, baud rate and address')), milliseconds);
    })]);
  } catch (error) {
    void closeSerial(s).catch(() => {});
    throw error;
  } finally { clearTimeout(timer); }
}
export async function writeSerial(s, bytes) {
  if (s.closed || !s.writer) throw new Error('Radio disconnected');
  await bounded(s, s.writer.write(bytes), 2000);
}
export async function readSerial(s, milliseconds) {
  if (s.closed || !s.reader) throw new Error('Radio disconnected');
  const result = await bounded(s, s.reader.read(), Math.max(1, milliseconds));
  if (result.done) throw new Error('Serial input disconnected');
  return result.value;
}
export async function setPttLine(s, line, on) {
  if (s.closed || !s.port) throw new Error('PTT port disconnected');
  s.line = line;
  await bounded(s, s.port.setSignals({ [line]: on }), 2000);
}
// A timeout releases TX and closes the session, so late ACKs cannot acknowledge another command.
export function armRelease(s, off) {
  s.off = off.length ? off : null;
  clearTimeout(s.timer);
  s.timer = setTimeout(() => { void closeSerial(s).catch(() => {}); }, 30000);
}
export function disarmRelease(s) {
  clearTimeout(s.timer); s.timer = null; s.off = null;
}
export function isSerialOpen(s) { return !!s.writer && !s.closed; }
export function closeSerial(s) {
  if (s.closing) return s.closing;
  s.closed = true;
  globalThis.removeEventListener?.("pagehide", s.onExit);
  if (s.onDisconnect) navigator.serial.removeEventListener?.("disconnect", s.onDisconnect);
  clearTimeout(s.timer);
  s.closing = (async () => {
    let failure;
    const attempt = async operation => {
      let timer;
      try {
        await Promise.race([operation(), new Promise((_, reject) => {
          timer = setTimeout(() => reject(new Error("Serial cleanup timed out; check radio RX")), 750);
        })]);
      } catch (error) { failure = error; }
      finally { clearTimeout(timer); }
    };
    // Best effort. USB removal or page/process termination cannot guarantee release.
    if (s.line && s.port) {
      await attempt(() => s.port.setSignals({ [s.line]: false }));
    }
    if (s.off && s.writer) {
      await attempt(() => s.writer.write(s.off));
    }
    if (s.reader) {
      await attempt(() => s.reader.cancel());
      s.reader.releaseLock(); s.reader = null;
    }
    if (s.writer) {
      await attempt(() => s.writer.close());
      s.writer.releaseLock(); s.writer = null;
    }
    if (s.port?.readable || s.port?.writable) {
      await attempt(() => s.port.close());
    }
    if (failure) throw failure;
  })();
  return s.closing;
}

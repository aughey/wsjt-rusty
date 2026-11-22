// CQ-triggered automatic exchange with emulated radio/output hardware.
const { chromium } = require('playwright');
const assert = require('node:assert/strict');
const path = require('node:path');
(async () => {
  const browser = await chromium.launch({ headless: true, args: ['--no-sandbox', '--use-fake-device-for-media-stream', '--use-fake-ui-for-media-stream', `--use-file-for-fake-audio-capture=${path.resolve(__dirname, '../../../tests/fixtures/ft8-210703-133430.wav')}`] });
  try {
    const page = await browser.newPage({ viewport: { width: 1200, height: 1000 } });
    const errors = []; page.on('pageerror', e => errors.push(e.message));
    await page.addInitScript(() => {
      window.presentationPayload = message => {
        let hash = 2166136261;
        for (const c of message) hash = Math.imul(hash ^ c.charCodeAt(0), 16777619) >>> 0;
        return hash.toString(2).padStart(74, '0') + '001';
      };
      const OriginalWorker = Worker;
      window.priorityRequests = [];
      window.Worker = class extends OriginalWorker { constructor(...args) { super(...args); window.testWorker = this; } postMessage(data,...rest) { if(data.kind==='priorities') priorityRequests.push(data.frequencies); return super.postMessage(data,...rest); } };
      const now = Date.now.bind(Date); window.offset = 0; Date.now = () => now() + offset;
      window.setUtc = utc => { offset = utc - now(); };
      const enumerate = navigator.mediaDevices.enumerateDevices.bind(navigator.mediaDevices);
      navigator.mediaDevices.enumerateDevices = async () => [...await enumerate(), { kind: 'audiooutput', deviceId: 'radio-card', label: 'USB Radio Output' }];
      window.sinks = []; window.played = []; window.resumeCount = 0;
      const resume = AudioContext.prototype.resume;
      AudioContext.prototype.resume = function() { resumeCount++; return resume.call(this); };
      const setSink = AudioContext.prototype.setSinkId;
      AudioContext.prototype.setSinkId = async function(id) {
        sinks.push(id); await setSink.call(this, '');
        // Reproduce the reported failure: changing output suspends a running context.
        await this.suspend();
        Object.defineProperty(this, 'sinkId', { configurable: true, value: id });
      };
      window.outputGains = [];
      const createGain = AudioContext.prototype.createGain;
      AudioContext.prototype.createGain = function() { const node = createGain.call(this); outputGains.push(node); return node; };
      const makeSource = AudioContext.prototype.createBufferSource;
      AudioContext.prototype.createBufferSource = function() {
        const source = makeSource.call(this), start = source.start.bind(source);
        source.start = when => { played.push({ when, utc: Date.now() + (when-this.currentTime)*1000,
          keyed: rig.signals.at(-1)?.requestToSend === true, length: source.buffer.length, toneHz: (()=>{const p=source.buffer.getChannelData(0);let n=0;for(let i=8000;i<18000;i++)if(p[i-1]<=0&&p[i]>0)n++;return n*12000/10000;})() }); start(when); }; return source;
      };
      window.rig = { writes: [], signals: [], opens: 0, closes: 0, frequency: 14074000, mode: 1, power: 25, reject: false };
      const serial = new EventTarget();
      serial.requestPort = async () => {
        let input;
        const port = {
          async open() {
            rig.opens++;
            this.readable = new ReadableStream({ start(c) { input = c; } });
            this.writable = new WritableStream({ write(bytes) {
              const b = [...bytes]; rig.writes.push(b);
              if (rig.reject) { input.enqueue(new Uint8Array([254,254,224,94,250,253])); return; }
              const response = [254,254,224,94];
              switch (b[4]) {
                case 3: {
                  let n = rig.frequency; response.push(3);
                  for (let i = 0; i < 5; i++) { response.push((Math.floor(n / 10) % 10) * 16 + n % 10); n = Math.floor(n / 100); }
                  break;
                }
                case 4: response.push(4, rig.mode, 1); break;
                case 5: {
                  rig.frequency = b.slice(5, 10).reduce((sum,v,i) => sum + ((v >> 4) * 10 + (v & 15)) * 100 ** i, 0);
                  response.push(251); break;
                }
                case 6: rig.mode = b[5]; response.push(251); break;
                case 20:
                  if (b.length > 7) { rig.power = b[6] * 100 + (b[7] >> 4) * 10 + (b[7] & 15); response.push(251); }
                  else { response.push(20,10,Math.floor(rig.power / 100), Math.floor(rig.power % 100 / 10) * 16 + rig.power % 10); }
                  break;
                case 28: response.push(251); break;
                default: throw new Error(`Unexpected command ${b}`);
              }
              response.push(253);
              input.enqueue(new Uint8Array(b)); // interface echo
              input.enqueue(new Uint8Array(response.slice(0,3)));
              input.enqueue(new Uint8Array(response.slice(3)));
            } });
          },
          async setSignals(signals) { rig.signals.push(signals); },
          async close() { rig.closes++; this.readable = null; this.writable = null; },
        };
        return port;
      };
      Object.defineProperty(navigator, 'serial', { value: serial });
    });
    const url = process.argv[2] || 'http://127.0.0.1:8083';
    await page.goto(url);
    const radio = page.getByRole('region', { name: 'IC-718 radio control' });
    await radio.getByLabel('PTT method').selectOption('cat-rts');
    await radio.getByRole('button', { name: 'Connect radio', exact: true }).click();
    await radio.getByText('IC-718 connected; polling every 2 seconds', { exact: true }).waitFor();
    await page.getByRole('button', { name: 'Start listening', exact: true }).click();
    await page.getByRole('status').filter({ hasText: 'Listening for FT8' }).waitFor({ timeout: 35000 });
    await page.locator('.conversation').first().waitFor({ timeout: 60000 });
    async function send(start, messages, incremental = false) {
      await page.evaluate(({start,messages,incremental}) => {
        if(incremental) {
          testWorker.dispatchEvent(new MessageEvent('message',{data:{kind:'decode-started'}}));
          for(const message of messages) testWorker.dispatchEvent(new MessageEvent('message',{data:{kind:'decode-found',decode:{start_utc_ms:start,end_utc_ms:start+12640,frequency_hz:1200,snr_db:-12,message,payload:presentationPayload(message)}}}));
          return;
        }
        testWorker.dispatchEvent(new MessageEvent('message', {data: {kind:'update', update: {
        capture_start_ms:start, capture_end_ms:start+13000, received_seconds:13, next_attempt_utc_ms:start+28500,
        decodes:messages.map(message => ({start_utc_ms:start,end_utc_ms:start+12640,frequency_hz:1200,snr_db:-12,message,payload:presentationPayload(message)})),
      }}})); }, {start,messages,incremental});
    }
    const epoch = Math.floor(Date.now()/15000)*15000;
    await send(epoch-30000+500, ['K2XYZ K1ABC -10', 'K3XYZ N0XYZ -10']);
    await page.locator('.conversation').filter({hasText:'K1ABC ↔ K2XYZ'}).click();
    await page.locator('.conversation').filter({hasText:'N0XYZ ↔ K3XYZ'}).click();
    const panel = page.getByRole('region', {name:'Stations to contact', exact:true});
    await panel.locator('[data-station="K1ABC"]').getByRole('button', {name:'Move to back',exact:true}).click();
    await panel.getByLabel('My callsign', {exact:true}).fill('W9XYZ');
    await panel.getByLabel('My grid', {exact:true}).fill('EN37');
    await panel.getByLabel('Transmit audio output', {exact:true}).selectOption('radio-card');
    await panel.getByLabel('TX audio attenuation', {exact:true}).fill('12');
    await panel.getByRole('button', {name:'Start contacting',exact:true}).click();
    await panel.getByText('CQ watch enabled', {exact:true}).waitFor();
    assert.equal(await page.evaluate(() => resumeCount >= 2), true, 'output resumes after sink suspension');
    await page.waitForTimeout(500);
    assert.deepEqual(await page.evaluate(() => played), []);
    await page.evaluate(epoch => setUtc(epoch+13800), epoch);
    await send(epoch+500, ['CQ K1ABC FN42', 'CQ N0XYZ EM00'], true);
    await page.waitForFunction(() => played.length === 1, null, {timeout:10000});
    await panel.locator('[data-station="N0XYZ"].active').waitFor();
    assert.deepEqual(await page.evaluate(() => priorityRequests.at(-1)), [1200]);
    assert.match(await panel.locator('[data-station="N0XYZ"] .station-cq').innerText(), /Last CQ: (just now|.* ago)/);
    assert.match(await panel.locator('[data-station="N0XYZ"] .station-cq-delay').innerText(), /before reply deadline/);
    assert.equal(await page.evaluate(() => played[0].keyed), true);
    assert.equal(await page.evaluate(() => played[0].length), 151680);
    assert.ok(await page.evaluate(() => played[0].toneHz > 1200 && played[0].toneHz < 1250), 'audio tones match received 1200 Hz, not an old fixed 1500 Hz');
    assert.deepEqual(await page.evaluate(() => sinks), ['radio-card']);
    const card = panel.locator('[data-station="N0XYZ"]');
    assert.equal(await card.locator('.exchange-slot').count(), 6);
    assert.match(await card.locator('.exchange-slot.received').first().innerText(), /CQ N0XYZ EM00/);
    assert.ok(Math.abs(await page.evaluate(() => outputGains.at(-1).gain.value) - 10**(-12/20)) < 0.0001);
    await panel.getByLabel('TX audio attenuation', {exact:true}).fill('20');
    await page.waitForFunction(() => Math.abs(outputGains.at(-1).gain.value - 0.1) < 0.0001);
    assert.equal(await page.evaluate(() => played.length), 1, 'live attenuation keeps the same transmission');
    assert.equal(await page.evaluate(() => rig.signals.at(-1).requestToSend), true);

    await page.waitForFunction(() => rig.signals.length >= 3 && rig.signals.at(-1).requestToSend === false, null, {timeout:18000});
    assert.equal(await page.evaluate(() => rig.closes), 0);
    await panel.getByRole('status').filter({hasText:'FT8 audio complete'}).waitFor();
    const firstStart = await page.evaluate(() => played[0].utc);
    const nextStart = Math.round(firstStart/15000)*15000 + 30500;
    await page.evaluate(now => setUtc(now), nextStart - 1700);
    await send(nextStart - 15000, ['W9XYZ N0XYZ -09']);
    await page.waitForFunction(() => played.length === 2, null, {timeout:10000});
    await panel.locator('.station-tx-status').filter({hasText:'N0XYZ W9XYZ R-12'}).waitFor();
    assert.match(await card.locator('.exchange-slot.received').last().innerText(), /W9XYZ N0XYZ -09/);
    assert.equal(await card.locator('.exchange-slot.missing').count(), 1, 'locally sent locator stays distinct from received evidence');
    assert.deepEqual(await page.evaluate(() => sinks), ['radio-card'], 'automatic replies reuse the user-primed audio context');
    await panel.locator('[data-station="N0XYZ"]').getByRole('button', {name:'Move to back',exact:true}).click();
    await page.waitForFunction(() => rig.signals.at(-1).requestToSend === false);
    assert.deepEqual(await panel.locator('.queued-station').evaluateAll(es=>es.map(e=>e.dataset.station)), ['K1ABC','N0XYZ']);
    await panel.locator('[data-station="N0XYZ"]').getByRole('button', {name:'Remove',exact:true}).click();
    await panel.getByRole('button', {name:'Stop contacting',exact:true}).click();
    await page.getByRole('button', {name:'Stop',exact:true}).click();
    await page.getByRole('status').filter({hasText:'Final FT8 decode complete'}).waitFor({timeout:60000});
    await page.reload();
    assert.equal(await panel.getByLabel('My callsign',{exact:true}).inputValue(),'W9XYZ');
    assert.equal(await panel.getByLabel('My grid',{exact:true}).inputValue(),'EN37');
    assert.equal(await panel.getByLabel('TX audio attenuation',{exact:true}).inputValue(),'20');
    assert.equal(await panel.getByLabel('Transmit audio output',{exact:true}).inputValue(),'radio-card');
    assert.equal(await panel.getByRole('button',{name:'Start contacting',exact:true}).isDisabled(),true);
    assert.deepEqual(errors, []);
    console.log('PASS: live output attenuation and saved level, queued exchange stages, sink suspension recovery, CQ priority, automatic replies, radio key/unkey and cancellation');
  } finally { await browser.close(); }
})().catch(e=>{console.error(e);process.exitCode=1;});

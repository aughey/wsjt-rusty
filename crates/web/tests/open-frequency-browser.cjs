// Release-browser regression with a real FT8 fixture and emulated audio output.
// NODE_PATH=<Playwright installation>/node_modules node this-file.cjs <preview URL>
const { chromium } = require('playwright');
const assert = require('node:assert/strict');
const path = require('node:path');

(async () => {
  const browser = await chromium.launch({ channel: process.env.WSJT_BROWSER_CHANNEL || 'chrome', headless: true, args: [
    '--use-fake-device-for-media-stream', '--use-fake-ui-for-media-stream',
    `--use-file-for-fake-audio-capture=${path.resolve(__dirname, '../../../tests/fixtures/ft8-210703-133430.wav')}`,
  ] });
  try {
    const page = await browser.newPage({ viewport: { width: 1200, height: 1000 } });
    const errors = [];
    page.on('pageerror', e => errors.push(e.message));
    await page.addInitScript(() => {
      const OriginalWorker = Worker;
      window.Worker = class extends OriginalWorker {
        constructor(...args) { super(...args); window.testWorker = this; }
      };
      window.streams = [];
      const getUserMedia = navigator.mediaDevices.getUserMedia.bind(navigator.mediaDevices);
      navigator.mediaDevices.getUserMedia = async (...args) => {
        const stream = await getUserMedia(...args); streams.push(stream); return stream;
      };
      const setSink = AudioContext.prototype.setSinkId;
      AudioContext.prototype.setSinkId = function() { return setSink.call(this, ''); };
      window.played = [];
      const createSource = AudioContext.prototype.createBufferSource;
      AudioContext.prototype.createBufferSource = function() {
        const source = createSource.call(this), start = source.start.bind(source);
        source.start = when => {
          const pcm = source.buffer.getChannelData(0);
          let crossings = 0;
          for (let i = 8000; i < 18000; i++) if (pcm[i - 1] <= 0 && pcm[i] > 0) crossings++;
          played.push(crossings * 12000 / 10000);
          start(when);
        };
        return source;
      };
    });
    await page.goto(process.argv[2] || 'http://127.0.0.1:8089');
    const panel = page.getByRole('region', { name: 'Stations to contact', exact: true });
    const checkbox = panel.getByLabel('find an open frequency', { exact: true });
    assert.equal(await checkbox.isChecked(), false);
    await checkbox.check();
    await page.reload();
    assert.equal(await checkbox.isChecked(), true, 'preference survives reload');
    assert.equal(await page.locator('.tx-frequency-marker').count(), 0, 'restoration does not schedule TX');
    await page.getByRole('button', { name: 'Start listening', exact: true }).click();
    await page.locator('.conversation').first().waitFor({ timeout: 60000 });
    await panel.getByLabel('My callsign', { exact: true }).fill('W9XYZ');
    await panel.getByLabel('My grid', { exact: true }).fill('EN37');
    await panel.getByLabel('Transmit audio output', { exact: true }).selectOption('default');
    await panel.getByLabel('Use connected radio PTT').uncheck();
    await page.evaluate(() => {
      const start = Math.floor(Date.now() / 15000) * 15000 - 29500;
      testWorker.dispatchEvent(new MessageEvent('message', { data: { kind: 'update', update: {
        capture_start_ms: start, capture_end_ms: start + 13000, received_seconds: 13,
        next_attempt_utc_ms: start + 28500,
        decodes: [{ start_utc_ms: start, end_utc_ms: start + 12640, frequency_hz: 1200,
          snr_db: -12, message: 'CQ K1ABC FN42', payload: '0'.repeat(74) + '001' }],
      } } }));
    });
    await page.locator('.conversation').filter({ hasText: 'K1ABC' }).first().click();
    await panel.getByRole('button', { name: 'Start contacting', exact: true }).click();
    // Use the actual UTC boundary so spectrum age and capture clock stay aligned.
    await page.waitForFunction(() => Date.now() % 15000 >= 13800 && Date.now() % 15000 <= 14200, null, { timeout: 20000 });
    await page.evaluate(() => {
      const start = Math.floor(Date.now() / 15000) * 15000 + 500;
      testWorker.dispatchEvent(new MessageEvent('message', { data: { kind: 'update', update: {
        capture_start_ms: start, capture_end_ms: start + 13000, received_seconds: 13,
        next_attempt_utc_ms: start + 28500,
        decodes: [{ start_utc_ms: start, end_utc_ms: start + 12640, frequency_hz: 1200,
          snr_db: -12, message: 'CQ K1ABC FN42', payload: '0'.repeat(74) + '001' }],
      } } }));
    });
    const card = panel.locator('[data-station="K1ABC"]');
    await card.getByText(/held open-frequency choice/).waitFor({ timeout: 2000 });
    const text = await card.locator('.station-match').innerText();
    const hz = Number(text.match(/TX ([\d.]+) Hz/)[1]);
    assert.ok(hz >= 110 && hz <= 3940, text);
    assert.match(await page.locator('.tx-frequency-marker').getAttribute('aria-label'), new RegExp(`${hz.toFixed(1)} audio Hz`));
    await page.waitForFunction(() => played.length > 0, { timeout: 5000 });
    const measured = await page.evaluate(() => played[0]);
    assert.ok(measured >= hz - 3 && measured <= hz + 47, `PCM ${measured} Hz matches selected FT8 footprint ${hz}`);
    await checkbox.uncheck();
    assert.match(await card.locator('.station-match').innerText(), new RegExp(`TX ${hz.toFixed(1)} Hz.*held`));
    await page.screenshot({ path: path.resolve('target/open-frequency-desktop.png'), fullPage: true });
    await page.setViewportSize({ width: 390, height: 844 });
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true, 'mobile has no horizontal overflow');
    assert.equal(await page.locator('.tx-frequency-marker span').evaluate(el => { const label = el.getBoundingClientRect(), plot = el.closest('.waterfall-plot').getBoundingClientRect(); return label.left >= plot.left && label.right <= plot.right; }), true, 'full TX frequency label stays inside mobile waterfall');
    await page.screenshot({ path: path.resolve('target/open-frequency-mobile.png'), fullPage: true });
    await panel.getByRole('button', { name: 'Stop contacting', exact: true }).click();
    assert.equal(await page.locator('.tx-frequency-marker').count(), 0);
    await page.getByRole('button', { name: 'Stop', exact: true }).click();
    await page.waitForFunction(() => streams.every(s => s.getTracks().every(t => t.readyState === 'ended')));
    await page.getByRole('button', { name: 'Start listening', exact: true }).waitFor({ timeout: 60000 });
    await page.getByRole('button', { name: 'Start listening', exact: true }).click();
    await page.getByRole('status').filter({ hasText: 'Listening for FT8' }).waitFor({ timeout: 35000 });
    assert.equal(await page.locator('.tx-frequency-marker').count(), 0, 'restart clears active TX placement');
    await page.getByRole('button', { name: 'Stop', exact: true }).click();
    assert.deepEqual(errors, []);
    console.log(JSON.stringify({ preferenceReload: true, selectedHz: hz, measuredToneHz: measured, mobile: true, stopRestart: true }));
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exit(1); });

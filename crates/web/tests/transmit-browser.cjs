// Browser queue controls and audio-only scheduling with a real simulated microphone.
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
      const WorkerClass = Worker;
      window.Worker = class extends WorkerClass { constructor(...args) { super(...args); window.testWorker = this; } };
      window.outputs = [];
      const createSource = AudioContext.prototype.createBufferSource;
      AudioContext.prototype.createBufferSource = function() {
        const source = createSource.call(this); const start = source.start.bind(source), stop = source.stop.bind(source);
        const row = { stopped: false }; outputs.push(row);
        source.start = when => { row.when = when; row.length = source.buffer.length; row.rate = source.buffer.sampleRate; start(when); };
        source.stop = () => { row.stopped = true; stop(); }; return source;
      };
    });
    await page.goto(process.argv[2] || 'http://127.0.0.1:8083');
    await page.getByRole('button', { name: 'Start listening', exact: true }).click();
    await page.getByRole('status').filter({ hasText: 'Listening for FT8' }).waitFor({ timeout: 35000 });
    await page.locator('.conversation').first().waitFor({ timeout: 60000 });
    assert.ok(await page.locator('tbody tr').count() > 0);
    async function send(start, messages) {
      await page.evaluate(({ start, messages }) => testWorker.dispatchEvent(new MessageEvent('message', { data: { kind: 'update', update: {
        capture_start_ms: start, capture_end_ms: start + 13000, received_seconds: 13, next_attempt_utc_ms: start + 28500,
        decodes: messages.map(message => ({ start_utc_ms: start, end_utc_ms: start + 12640, frequency_hz: 1200, snr_db: -12, message, payload: presentationPayload(message) })),
      } } })), { start, messages });
    }
    const epoch = Math.floor(Date.now() / 15000) * 15000;
    await send(epoch - 30000 + 500, ['K2XYZ K1ABC -10', 'K3XYZ N0XYZ -11']);
    await page.locator('.conversation').filter({ hasText: 'K1ABC ↔ K2XYZ' }).click();
    await page.locator('.conversation').filter({ hasText: 'N0XYZ ↔ K3XYZ' }).focus();
    await page.keyboard.press('Enter');
    const panel = page.getByRole('region', { name: 'Stations to contact', exact: true });
    assert.deepEqual(await panel.locator('.queued-station').evaluateAll(es => es.map(e => e.dataset.station)), ['K1ABC', 'N0XYZ']);
    assert.equal(await page.locator('.conversation').filter({ hasText: 'K1ABC ↔ K2XYZ' }).count(), 0);
    await panel.locator('[data-station="K1ABC"]').getByRole('button', { name: 'Move to back', exact: true }).click();
    assert.deepEqual(await panel.locator('.queued-station').evaluateAll(es => es.map(e => e.dataset.station)), ['N0XYZ', 'K1ABC']);
    await panel.getByLabel('My callsign', { exact: true }).fill('W9XYZ');
    await panel.getByLabel('My grid', { exact: true }).fill('EN37');
    await panel.getByLabel('Use connected radio PTT').uncheck();
    await panel.getByLabel('Transmit audio output', { exact: true }).selectOption('default');
    await panel.getByRole('button', { name: 'Start contacting', exact: true }).click();
    await panel.getByText('CQ watch enabled', { exact: true }).waitFor();
    await page.waitForTimeout(500);
    assert.equal(await page.evaluate(() => outputs.length), 0, 'mid-contact and stale evidence cannot initiate TX');
    await send(Math.floor(Date.now()/15000)*15000 + 500, ['CQ K1ABC FN42', 'CQ N0XYZ EM00']);
    await page.waitForFunction(() => outputs.some(o => o.when !== undefined), null, { timeout: 18000 });
    await panel.locator('[data-station="N0XYZ"].active').waitFor();
    assert.equal(await page.evaluate(() => outputs[0].length), 151680);
    assert.equal(await page.evaluate(() => outputs[0].rate), 12000);
    const station = panel.locator('[data-station="N0XYZ"]');
    await station.locator('.transmit-steps li.transmitting').waitFor({ timeout: 8000 });
    assert.equal(await station.evaluate(e => e.classList.contains('transmitting')), true);
    assert.match(await station.locator('.station-tx-status').innerText(), /TX AUDIO ON · N0XYZ W9XYZ EN37/);
    assert.equal(await station.locator('.transmit-steps li.sent').count(), 0);
    assert.equal(await station.locator('.transmit-steps li.transmitting').evaluate(e => getComputedStyle(e).animationName), 'tx-glow');
    await page.emulateMedia({ reducedMotion: 'reduce' });
    assert.equal(await station.locator('.transmit-steps li.transmitting').evaluate(e => getComputedStyle(e).animationName), 'none');
    await page.emulateMedia({ reducedMotion: 'no-preference' });
    if (process.env.WSJT_BROWSER_SCREENSHOT) await panel.screenshot({ path: process.env.WSJT_BROWSER_SCREENSHOT.replace('.png', '-active.png') });
    await station.locator('.transmit-steps li.sent').waitFor({ timeout: 18000 });
    assert.equal(await station.locator('.transmit-steps li.transmitting').count(), 0);
    assert.match(await station.locator('.transmit-steps li.sent').innerText(), /Sent audio[\s\S]*N0XYZ W9XYZ EN37/);
    await panel.getByRole('button', { name: 'Stop contacting', exact: true }).click();
    await page.waitForFunction(() => outputs.every(o => o.stopped));
    assert.equal(await station.locator('.transmit-steps li.sent').count(), 1, 'Stop retains completed local playback');
    if (process.env.WSJT_BROWSER_SCREENSHOT) await panel.screenshot({ path: process.env.WSJT_BROWSER_SCREENSHOT.replace('.png', '-sent.png') });
    await panel.locator('[data-station="N0XYZ"]').getByRole('button', { name: 'Remove', exact: true }).click();
    assert.equal(await panel.locator('.queued-station').count(), 1);
    await page.setViewportSize({ width: 375, height: 812 });
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
    if (process.env.WSJT_BROWSER_SCREENSHOT) await panel.screenshot({ path: process.env.WSJT_BROWSER_SCREENSHOT });
    await page.getByRole('button', { name: 'Stop', exact: true }).click();
    await page.getByRole('status').filter({ hasText: 'Final FT8 decode complete' }).waitFor({ timeout: 60000 });
    await page.getByRole('button', { name: 'Start listening', exact: true }).click();
    await page.getByRole('status').filter({ hasText: 'Listening for FT8' }).waitFor({ timeout: 35000 });
    assert.equal(await panel.locator('.queued-station').count(), 1, 'selected stations survive capture restart');
    await panel.getByText('Paused', { exact: true }).waitFor();
    await page.getByRole('button', { name: 'Stop', exact: true }).click();
    await page.getByRole('status').filter({ hasText: 'Final FT8 decode complete' }).waitFor({ timeout: 60000 });
    assert.deepEqual(errors, []);
    console.log('PASS: live fixture, moving cards to station queue, keyboard selection, priority/removal, CQ-only audio trigger, cancellation, mobile layout and Stop/restart');
  } finally { await browser.close(); }
})().catch(e => { console.error(e); process.exitCode = 1; });

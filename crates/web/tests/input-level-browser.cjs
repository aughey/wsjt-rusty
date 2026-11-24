// Release-WASM input meter regression with recorded audio, no physical device.
// NODE_PATH=target/browser-qa/node_modules node crates/web/tests/input-level-browser.cjs URL
const { chromium } = require('playwright');
const assert = require('node:assert/strict');
const path = require('node:path');

(async () => {
  const browser = await chromium.launch({
    channel: process.env.WSJT_BROWSER_CHANNEL || 'chromium', headless: true,
    args: ['--use-fake-device-for-media-stream', '--use-fake-ui-for-media-stream',
      `--use-file-for-fake-audio-capture=${path.resolve(__dirname, '../../../tests/fixtures/ft8-210703-133430.wav')}`],
  });
  try {
    const page = await browser.newPage({ viewport: { width: 1200, height: 900 } });
    const errors = [];
    page.on('pageerror', error => errors.push(error.message));
    await page.addInitScript(() => {
      window.inputNodes = []; window.inputStreams = []; window.inputContexts = [];
      const NativeNode = AudioWorkletNode;
      window.AudioWorkletNode = class extends NativeNode {
        constructor(...args) {
          super(...args); inputNodes.push(this);
          // Reproduce a live waterfall with missing control-port peak events.
          this.port.addEventListener('message', event => {
            if (event.data.kind === 'input-level') event.stopImmediatePropagation();
          });
        }
      };
      const NativeContext = AudioContext;
      window.AudioContext = class extends NativeContext {
        constructor(...args) { super(...args); inputContexts.push(this); }
      };
      const media = navigator.mediaDevices.getUserMedia.bind(navigator.mediaDevices);
      navigator.mediaDevices.getUserMedia = async options => {
        const stream = await media(options); inputStreams.push(stream); return stream;
      };
    });
    await page.goto(process.argv[2] || 'http://127.0.0.1:8113');
    await page.getByRole('tab', { name: 'Spectrum', exact: true }).click();
    const value = page.locator('.input-peak-value');
    await value.filter({ hasText: 'Waiting for audio' }).waitFor();
    const start = page.getByRole('button', { name: 'Start listening', exact: true });
    const stop = page.getByRole('button', { name: 'Stop', exact: true });
    await start.click();
    try {
      await page.waitForFunction(() => document.querySelector('#input-peak').value > 0);
    } catch (error) {
      console.error('Input state:', await value.innerText(), await page.locator('.session-bar').innerText(),
        await page.evaluate(() => ({ contexts: inputContexts.map(c => ({state:c.state, time:c.currentTime})), nodes:inputNodes.length })), errors);
      throw error;
    }
    assert.match(await value.innerText(), /^0\.\d{3} \/ 1$/);
    // Live receive results must still arrive through the unchanged decoder path.
    await page.getByRole('tab', { name: 'Decoded messages', exact: true }).click();
    await page.locator('tbody tr').first().waitFor({ timeout: 60000 });
    await page.getByRole('tab', { name: 'Spectrum', exact: true }).click();
    // Exercise display thresholds independently of the recording's gain.
    await page.evaluate(() => {
      window.deliverLevel = inputNodes[0].port.onmessage;
      inputNodes[0].port.onmessage = event => {
        if (event.data.kind !== 'input-level') deliverLevel(event);
      };
    });
    for (const [peak, clipping, text] of [[0, false, '0.000'], [0.99999, false, '0.999'], [1, true, '1.000'], [1.2, true, '1.000']]) {
      await page.evaluate(peak => deliverLevel({ data: { kind: 'input-level', peak } }), peak);
      assert.equal(await page.locator('.input-level').evaluate(e => e.classList.contains('clipping')), clipping);
      assert.ok((await value.innerText()).startsWith(`${text} / 1`));
    }
    await page.setViewportSize({ width: 390, height: 844 });
    assert.ok(await page.locator('.input-level').evaluate(e => e.scrollWidth <= e.clientWidth));
    await stop.click();
    await page.waitForFunction(() => !document.querySelector('.session-bar button').disabled);
    const frozen = await value.innerText();
    await page.waitForTimeout(400);
    assert.equal(await value.innerText(), frozen);
    assert.ok(await page.evaluate(() => inputStreams.every(s => s.getTracks().every(t => t.readyState === 'ended'))));
    assert.ok(await page.evaluate(() => inputContexts.every(c => c.state === 'closed')));
    await start.click();
    await value.filter({ hasText: 'Waiting for audio' }).waitFor();
    await page.waitForFunction(() => document.querySelector('#input-peak').value > 0);
    await stop.click();
    await page.waitForFunction(() => !document.querySelector('.session-bar button').disabled);
    assert.ok(await page.evaluate(() => inputStreams.every(s => s.getTracks().every(t => t.readyState === 'ended'))));
    assert.deepEqual(errors, []);
    console.log('PASS: snapshot fallback without peak events, FT8 emission, clipping thresholds, narrow layout, Stop/restart and input release');
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });

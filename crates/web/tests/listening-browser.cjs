// Release-WASM receive lifecycle regression; uses recorded audio, no real radio.
// NODE_PATH=target/browser-qa/node_modules node this-file.cjs URL
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
      window.receiveContexts = [];
      window.receiveStreams = [];
      window.receiveWorkers = [];
      window.stressPriorities = false;
      window.receiveAttempts = [];
      const NativeWorker = Worker;
      window.Worker = class extends NativeWorker {
        constructor(...args) {
          super(...args); receiveWorkers.push(this);
          this.addEventListener('message', ({data}) => {
            if (data.kind === 'decode-completed') receiveAttempts.push({lane: this.role, ms: data.elapsed_ms});
          });
        }
        postMessage(data, ...args) {
          if (data.role) this.role = data.role;
          if (data.kind === 'priorities' && stressPriorities) {
            data = {...data, frequencies: Array.from({length:100}, (_,i) => 100+i*39)};
          }
          return super.postMessage(data, ...args);
        }
      };
      const NativeAudio = AudioContext;
      window.AudioContext = class extends NativeAudio {
        constructor(...args) { super(...args); receiveContexts.push(this); }
      };
      const getUserMedia = navigator.mediaDevices.getUserMedia.bind(navigator.mediaDevices);
      navigator.mediaDevices.getUserMedia = async options => {
        const stream = await getUserMedia(options); receiveStreams.push(stream); return stream;
      };
    });
    let navigations = 0;
    page.on('framenavigated', frame => { if (frame === page.mainFrame()) navigations++; });
    const response = await page.goto(process.argv[2] || 'http://127.0.0.1:8107');
    assert.ok(!(await response.text()).includes('.well-known/trunk/ws'), 'Trunk must not inject automatic reloads into a receive session');
    const start = page.getByRole('button', { name: 'Start listening', exact: true });
    const stop = page.getByRole('button', { name: 'Stop', exact: true });
    const status = page.locator('.session-bar .status');
    await start.click();
    await status.filter({ hasText: 'Listening for FT8' }).waitFor();
    await page.getByRole('tab', { name: 'Conversations', exact: true }).click();
    try {
      await page.locator('.conversation-list > .conversation').first().waitFor({ timeout: 60000 });
    } catch (error) {
      console.error('Receive state:', await status.innerText(), await page.evaluate(() => receiveContexts.map(c => ({ state: c.state, time: c.currentTime, rate: c.sampleRate, sink: c.sinkId.type }))), errors);
      throw error;
    }
    if (process.env.WSJT_STRESS_PRIORITIES) {
      await page.locator('.conversation-list > .conversation').first().click();
      await page.evaluate(() => {
        stressPriorities = true;
        for (const worker of receiveWorkers.filter(w => w.role)) worker.postMessage({kind:'priorities', frequencies:[]});
      });
    }
    // Cross several receive attempts with real WASM and the live audio clock.
    const until = Date.now() + Number(process.env.WSJT_LISTENING_SECONDS || 45) * 1000;
    while (Date.now() < until) {
      assert.equal(await start.isDisabled(), true, await status.innerText());
      assert.equal(await page.locator('.receive-error').count(), 0);
      await page.waitForTimeout(1000);
    }
    assert.ok(await page.evaluate(() => receiveContexts[0].currentTime > 45));
    assert.equal(navigations, 1, 'the receive page must stay loaded throughout listening');
    console.log('Completed live decode attempts:', await page.evaluate(() => receiveAttempts));
    // A true clock gap remains explicit, with a readable reason on narrow views.
    await page.evaluate(() => receiveContexts[0].suspend());
    const alert = page.locator('.receive-error');
    await alert.filter({ hasText: 'Audio clock suspended' }).waitFor();
    await start.waitFor({ state: 'visible' });
    await page.waitForFunction(() => !document.querySelector('.session-bar button').disabled);
    await page.setViewportSize({ width: 390, height: 844 });
    assert.ok(await alert.evaluate(element => element.scrollWidth <= element.clientWidth));
    assert.ok(await page.evaluate(() => receiveStreams.every(s => s.getTracks().every(t => t.readyState === 'ended'))));
    await start.click();
    await status.filter({ hasText: 'Listening for FT8' }).waitFor();
    assert.equal(await alert.count(), 0);
    await stop.click();
    await status.filter({ hasText: 'Final FT8 decode complete' }).waitFor({ timeout: 60000 });
    assert.ok(await page.evaluate(() => receiveContexts.every(c => c.state === 'closed')));
    assert.ok(await page.evaluate(() => receiveStreams.every(s => s.getTracks().every(t => t.readyState === 'ended'))));
    assert.deepEqual(errors, []);
    console.log('PASS: sustained live decoding, visible interruption, restart and release');
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });

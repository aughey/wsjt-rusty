// NODE_PATH=/path/to/node_modules node crates/web/tests/radio-browser.cjs URL
const { chromium } = require('playwright');
const assert = require('node:assert/strict');
const path = require('node:path');
(async () => {
  const browser = await chromium.launch({ headless: true, args: ['--no-sandbox', '--use-fake-device-for-media-stream', '--use-fake-ui-for-media-stream', `--use-file-for-fake-audio-capture=${path.resolve(__dirname, '../../../tests/fixtures/ft8-210703-133430.wav')}`] });
  try {
    const page = await browser.newPage({ viewport: { width: 1200, height: 1000 } });
    const errors = [];
    page.on('pageerror', error => errors.push(error.message));
    await page.addInitScript(() => {
      window.capturedStreams = [];
      const getUserMedia = navigator.mediaDevices.getUserMedia.bind(navigator.mediaDevices);
      navigator.mediaDevices.getUserMedia = async constraints => { const stream = await getUserMedia(constraints); capturedStreams.push(stream); return stream; };
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
              const respond = () => {
                input.enqueue(new Uint8Array(b)); // interface echo
                input.enqueue(new Uint8Array(response.slice(0,3)));
                input.enqueue(new Uint8Array(response.slice(3)));
                rig.pendingRead = false;
              };
              if ([3,4].includes(b[4]) && rig.readDelay) { rig.pendingRead = true; setTimeout(respond, rig.readDelay); }
              else respond();
            } });
          },
          async setSignals(signals) { rig.signals.push(signals); },
          async close() { rig.closes++; this.readable = null; this.writable = null; },
        };
        return port;
      };
      Object.defineProperty(navigator, 'serial', { value: serial });
    });
    await page.goto(process.argv[2] || 'http://127.0.0.1:8082');
    const panel = page.getByRole('region', { name: 'IC-718 radio control' });
    await panel.getByLabel('Baud').selectOption('9600');
    await panel.getByLabel('CI-V hex').fill('5e');
    await panel.getByLabel('PTT method').selectOption('cat-rts');
    await panel.getByRole('button', { name: 'Connect radio', exact: true }).click();
    await panel.getByText('IC-718 connected; polling every 2 seconds', { exact: true }).waitFor();
    assert.match(await panel.locator('.radio-dial').innerText(), /14.074000 MHz.*USB/);
    assert.deepEqual(await page.evaluate(() => rig.signals[0]), { requestToSend: false });
    assert.equal(await page.evaluate(() => rig.writes.every(b => [3,4].includes(b[4]))), true, 'Connect only polls');
    // Slow read replies expose transient disabled-state changes over several polls.
    await page.evaluate(() => {
      rig.readDelay = 250;
      window.pollFlashes = [];
      const buttons = [...document.querySelectorAll('.radio-panel button')].filter(b => ['Set frequency', 'Set mode'].includes(b.textContent));
      window.pollObserver = new MutationObserver(mutations => { for (const m of mutations) pollFlashes.push(m.target.textContent); });
      buttons.forEach(b => pollObserver.observe(b, { attributes: true, attributeFilter: ['disabled'] }));
    });
    await page.waitForTimeout(6500);
    assert.deepEqual(await page.evaluate(() => pollFlashes), [], 'routine polls must not disable/enable operator controls');
    await page.evaluate(() => pollObserver.disconnect());
    await page.waitForFunction(() => rig.pendingRead);
    await panel.getByLabel('FT8 preset').selectOption('7074000');
    await panel.getByRole('button', { name: 'Set frequency', exact: true }).click();
    await page.waitForFunction(() => rig.frequency === 7074000);
    await panel.getByText('Radio synchronized', { exact: true }).waitFor();
    await page.evaluate(() => { rig.readDelay = 0; });
    await panel.getByRole('button', { name: 'Set frequency', exact: true }).waitFor({ state: 'visible' });
    await panel.getByLabel(/^Mode /).selectOption('0');
    await panel.getByRole('button', { name: 'Set mode', exact: true }).click();
    await page.waitForFunction(() => rig.mode === 0);
    await panel.getByLabel('RF power setting (0–255)').fill('50');
    await panel.getByRole('button', { name: 'Set RF power', exact: true }).click();
    await panel.getByText('Readback: 50 / 255 (not watts)', { exact: true }).waitFor();
    await panel.getByRole('button', { name: 'Transmit (30 s max)', exact: true }).click();
    await page.waitForFunction(() => rig.signals.some(s => s.requestToSend === true));
    assert.equal(await page.evaluate(() => rig.opens), 1, 'PTT reuses the CAT port');
    await panel.getByRole('button', { name: 'Receive / unkey', exact: true }).click();
    await page.waitForFunction(() => rig.closes === 1);
    assert.deepEqual(await page.evaluate(() => rig.signals.at(-1)), { requestToSend: false });
    await panel.getByRole('button', { name: 'Connect radio', exact: true }).click();
    await panel.getByText('IC-718 connected; polling every 2 seconds', { exact: true }).waitFor();
    await panel.getByRole('button', { name: 'Disconnect radio', exact: true }).click();
    await page.waitForFunction(() => rig.closes === 2);
    // A negative ACK reports failure and closes the session rather than reusing stale ACKs.
    await page.evaluate(() => { rig.reject = true; });
    await panel.getByRole('button', { name: 'Connect radio', exact: true }).click();
    await panel.getByText('Radio rejected the command', { exact: true }).waitFor();
    await page.waitForFunction(() => rig.closes === 3);
    await page.reload();
    assert.equal(await panel.getByLabel('Baud').inputValue(), '9600');
    assert.equal(await panel.getByLabel('CI-V hex').inputValue(), '5e');
    assert.equal(await panel.getByLabel('Dial frequency (Hz)').inputValue(), '7074000');
    assert.equal(await panel.getByLabel('FT8 preset').inputValue(), '7074000');
    assert.equal(await panel.getByLabel(/^Mode /).inputValue(), '0');
    assert.equal(await panel.getByLabel('RF power setting (0–255)').inputValue(), '50');
    assert.equal(await panel.getByLabel('PTT method').inputValue(), 'cat-rts');
    assert.equal(await page.evaluate(() => rig.opens), 0);
    assert.deepEqual(await page.evaluate(() => rig.writes), [], 'restoring settings does not connect, tune, change power or key');
    await page.setViewportSize({ width: 390, height: 844 });
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
    await page.getByRole('button', { name: 'Start listening', exact: true }).click();
    await page.getByRole('status').filter({ hasText: 'Listening for FT8' }).waitFor({ timeout: 35000 });
    await page.locator('tbody tr').first().waitFor({ timeout: 60000 });
    await page.getByRole('button', { name: 'Stop', exact: true }).click();
    await page.getByRole('status').filter({ hasText: 'Final FT8 decode complete' }).waitFor({ timeout: 60000 });
    assert.equal(await page.evaluate(() => capturedStreams.every(s => s.getTracks().every(t => t.readyState === 'ended'))), true);
    await page.getByRole('button', { name: 'Start listening', exact: true }).click();
    await page.getByRole('status').filter({ hasText: 'Listening for FT8' }).waitFor({ timeout: 35000 });
    await page.getByRole('button', { name: 'Stop', exact: true }).click();
    await page.getByRole('status').filter({ hasText: 'Final FT8 decode complete' }).waitFor({ timeout: 60000 });
    assert.equal(await page.evaluate(() => capturedStreams.every(s => s.getTracks().every(t => t.readyState === 'ended'))), true);
    await page.evaluate(() => localStorage.setItem('wsjt-rusty.radio.v1', JSON.stringify({ baud: 1, address: 'ZZ', frequency: '0', mode: 255, power: '999', method: 'invalid' })));
    await page.reload();
    assert.equal(await panel.getByLabel('Baud').inputValue(), '19200');
    assert.equal(await panel.getByLabel('CI-V hex').inputValue(), '5E');
    assert.equal(await panel.getByLabel('Dial frequency (Hz)').inputValue(), '14074000');
    assert.equal(await panel.getByLabel(/^Mode /).inputValue(), '1');
    assert.equal(await panel.getByLabel('RF power setting (0–255)').inputValue(), '25');
    assert.equal(await panel.getByLabel('PTT method').inputValue(), 'none');
    assert.deepEqual(await page.evaluate(() => rig.writes), []);
    assert.deepEqual(errors, []);
    console.log('Radio UI: tuning, modes, RF level, same-port RTS TX/RX, reconnect, NG mobile layout and independent live FT8 capture/Stop/restart passed');
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });

// NODE_PATH=/path/to/node_modules node crates/web/tests/tabs-browser.cjs URL
const { chromium } = require('playwright');
const assert = require('node:assert/strict');
const path = require('node:path');
(async () => {
  const browser = await chromium.launch({ channel: process.env.WSJT_BROWSER_CHANNEL || 'chrome', headless: true, args: [
    '--use-fake-device-for-media-stream', '--use-fake-ui-for-media-stream',
    `--use-file-for-fake-audio-capture=${path.resolve(__dirname, '../../../tests/fixtures/ft8-210703-133430.wav')}`,
  ] });
  try {
    const page = await browser.newPage({ viewport: { width: 1440, height: 1000 } });
    const errors = []; page.on('pageerror', e => errors.push(e.message));
    await page.addInitScript(() => {
      window.streams = []; window.contexts = []; window.waterfallPaints = 0;
      const media = navigator.mediaDevices.getUserMedia.bind(navigator.mediaDevices);
      navigator.mediaDevices.getUserMedia = async c => { const s = await media(c); streams.push(s); return s; };
      const setSink = AudioContext.prototype.setSinkId;
      AudioContext.prototype.setSinkId = function() { return setSink.call(this, ''); };
      const Audio = AudioContext;
      window.AudioContext = class extends Audio { constructor(...args) { super(...args); contexts.push(this); } };
      const put = CanvasRenderingContext2D.prototype.putImageData;
      CanvasRenderingContext2D.prototype.putImageData = function(...args) { if (this.canvas.id === 'waterfall') waterfallPaints++; return put.apply(this, args); };
    });
    await page.goto(process.argv[2] || 'http://127.0.0.1:8093');
    const tabs = page.getByRole('tablist', { name: 'Application views', exact: true });
    const select = async name => {
      const tab = tabs.getByRole('tab', { name, exact: true }); await tab.click();
      assert.equal(await tab.getAttribute('aria-selected'), 'true');
      assert.equal(await page.locator('#active-panel').getAttribute('aria-labelledby'), await tab.getAttribute('id'));
    };
    const status = async text => {
      try { await page.getByRole('status').filter({ hasText: text }).waitFor({ timeout: 60000 }); }
      catch (error) { console.error('Receive session:', await page.locator('.session-bar').innerText(), errors); throw error; }
    };
    await tabs.waitFor();
    assert.ok(await page.evaluate(() => document.querySelector('main').getBoundingClientRect().height <= innerHeight + 1), 'shell fills viewport');
    assert.equal(await tabs.getByRole('tab').count(), 8);
    await page.locator('#contact-map canvas').first().waitFor();
    await select('Contact configuration');
    assert.equal(await page.locator('#contact-map').count(), 0, 'hidden map is disposed');
    assert.equal(await page.locator('.station-queue').count(), 0, 'hidden cards are unmounted');
    await page.getByLabel('My callsign', { exact: true }).fill('W9XYZ');
    await page.getByLabel('My grid', { exact: true }).fill('EN37');
    await page.getByLabel('Use connected radio PTT').uncheck();
    await page.getByLabel('Transmit audio output', { exact: true }).selectOption('default');
    await select('Radio control'); assert.equal(await page.locator('.contact-configuration').count(), 0);
    assert.equal(await page.locator('.radio-panel').count(), 1);
    await select('Spectrum'); assert.equal(await page.locator('.radio-panel').count(), 0);
    await page.getByRole('button', { name: 'Start listening', exact: true }).click(); await status('Listening for FT8');
    await page.waitForFunction(() => waterfallPaints > 3);
    await select('Decoded messages'); const paints = await page.evaluate(() => waterfallPaints);
    await page.waitForTimeout(1200);
    assert.ok(await page.evaluate(() => waterfallPaints) > paints, 'hidden spectrum retains live history');
    assert.equal(await page.locator('.decode-box').count(), 0, 'hidden annotations are unmounted');
    await page.locator('tbody tr').first().waitFor({ timeout: 60000 });
    await select('Conversations'); assert.equal(await page.locator('tbody tr').count(), 0);
    const card = page.locator('.conversation-list > .conversation').first(); await card.waitFor(); await card.click();
    await select('Stations to contact'); await page.locator('.queued-station').first().waitFor();
    await page.locator('#tx-attenuation').fill('25');
    await page.screenshot({ path: path.resolve(__dirname, '../../../target/tabs-desktop.png'), fullPage: true });
    await page.waitForFunction(n => waterfallPaints > n, paints);
    assert.equal(await page.getByLabel('My callsign', { exact: true }).count(), 0);
    await page.getByRole('button', { name: 'Start contacting', exact: true }).click();
    await page.getByText('CQ watch enabled', { exact: true }).waitFor();
    await select('Contact configuration');
    await page.waitForTimeout(1200);
    assert.ok(await page.evaluate(() => contexts.slice(1).some(c => c.state === 'running')), 'contact output survives page navigation');
    assert.equal(await page.getByLabel('My callsign', { exact: true }).inputValue(), 'W9XYZ');
    assert.equal(await page.getByLabel('My callsign', { exact: true }).isEnabled(), false);
    await select('Stations to contact'); await page.getByText('CQ watch enabled', { exact: true }).waitFor();
    assert.equal(await page.locator('#tx-attenuation').inputValue(), '25');
    assert.equal(await page.getByRole('button', { name: 'Stop contacting', exact: true }).isDisabled(), false);
    await select('Log'); await page.locator('.contact-log').waitFor();
    await select('Spectrum'); assert.equal(await page.locator('.contact-log').count(), 0);
    await tabs.getByRole('tab', { name: 'Spectrum', exact: true }).focus(); await page.keyboard.press('ArrowRight');
    assert.equal(await tabs.getByRole('tab', { name: 'Map', exact: true }).getAttribute('aria-selected'), 'true');
    await page.keyboard.press('End'); assert.equal(await tabs.getByRole('tab', { name: 'Conversations', exact: true }).getAttribute('aria-selected'), 'true');
    await page.keyboard.press('Home'); assert.equal(await tabs.getByRole('tab', { name: 'Stations to contact', exact: true }).getAttribute('aria-selected'), 'true');
    await page.setViewportSize({ width: 390, height: 844 });
    for (const name of ['Stations to contact', 'Spectrum', 'Contact configuration', 'Conversations', 'Log']) {
      await select(name); assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), `overflow in ${name}`);
    }
    for (let attempt = 0; attempt < 2; attempt++) {
      if (attempt) { await page.getByRole('button', { name: 'Start listening', exact: true }).click(); await status('Listening for FT8'); }
      await page.getByRole('button', { name: 'Stop', exact: true }).click(); await status('Final FT8 decode complete');
      assert.ok(await page.evaluate(() => streams.every(s => s.getTracks().every(t => t.readyState === 'ended'))));
    }
    assert.deepEqual(errors, []);
    console.log('PASS: tabs, hidden work, live fixture, persistent contacting, settings, keyboard, mobile, Stop/restart');
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });

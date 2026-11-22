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
    const page = await browser.newPage({ viewport: { width: 1280, height: 1000 } });
    const errors = []; page.on('pageerror', e => errors.push(e.message));
    await page.addInitScript(() => {
      window.streams = [];
      const get = navigator.mediaDevices.getUserMedia.bind(navigator.mediaDevices);
      navigator.mediaDevices.getUserMedia = async (...args) => {
        const stream = await get(...args); streams.push(stream); return stream;
      };
    });
    await page.goto(process.argv[2] || 'http://127.0.0.1:8094');
    await page.getByRole('tab', { name: 'Spectrum', exact: true }).click();
    assert.equal(await page.locator('.timing-guide').count(), 0);
    await page.getByRole('button', { name: 'Start listening', exact: true }).click();
    await page.getByRole('status').filter({ hasText: 'Listening for FT8' }).waitFor({ timeout: 60000 }).catch(async e => {
      console.error(await page.locator('.session-bar').innerText(), errors); throw e;
    });
    await page.locator('.timing-guide:not(.end-guide)').first().waitFor({ timeout: 25000 });
    await page.locator('.timing-guide.end-guide').first().waitFor({ timeout: 25000 });
    const start = page.locator('.timing-guide:not(.end-guide)').first();
    const top = parseFloat(await start.evaluate(e => e.style.top));
    await page.waitForTimeout(1000);
    assert.ok(parseFloat(await start.evaluate(e => e.style.top)) > top, 'guides scroll with audio');
    const box = page.locator('.decode-box:visible').first();
    await box.waitFor({ timeout: 60000 });
    await box.focus();
    await page.getByRole('tooltip').getByText(/Start offset [+-]\d+\.\d{2} s/).waitFor();
    await page.screenshot({ path: 'target/timing-guides-desktop.png' });
    await page.setViewportSize({ width: 390, height: 844 });
    assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
    assert.ok(await page.getByRole('tooltip').evaluate(e => {
      const r = e.getBoundingClientRect();
      return r.top >= 0 && r.bottom <= innerHeight && r.left >= 0 && r.right <= innerWidth;
    }), 'mobile timing details fit the viewport');
    await page.screenshot({ path: 'target/timing-guides-mobile.png' });
    await page.getByRole('button', { name: 'Stop', exact: true }).click();
    await page.getByRole('status').filter({ hasText: 'Final FT8 decode complete' }).waitFor({ timeout: 60000 });
    const frozen = await page.locator('.timing-guides').innerHTML();
    await page.waitForTimeout(500);
    assert.equal(await page.locator('.timing-guides').innerHTML(), frozen);
    assert.ok(await page.evaluate(() => streams.every(s => s.getTracks().every(t => t.readyState === 'ended'))));
    await page.getByRole('button', { name: 'Start listening', exact: true }).click();
    await page.waitForTimeout(500);
    assert.ok(await page.locator('.timing-guide').count() <= 1, 'restart clears old guides');
    await page.getByRole('button', { name: 'Stop', exact: true }).click();
    await page.getByRole('status').filter({ hasText: 'Final FT8 decode complete' }).waitFor({ timeout: 60000 });
    assert.deepEqual(errors, []);
    console.log('PASS: timing guides, live fixture, decode offset, scrolling, mobile, freeze and restart');
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });

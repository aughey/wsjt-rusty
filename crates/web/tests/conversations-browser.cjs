// Optional browser integration check. Requires Playwright + Chromium.
// Serve a release Trunk build, then run:
// NODE_PATH=/path/to/node_modules node crates/web/tests/conversations-browser.cjs http://127.0.0.1:8082
const { chromium } = require('playwright');
const assert = require('node:assert/strict');
const path = require('node:path');

(async () => {
  const browser = await chromium.launch({ headless: true, args: [
    '--no-sandbox', '--use-fake-device-for-media-stream', '--use-fake-ui-for-media-stream',
    `--use-file-for-fake-audio-capture=${path.resolve(__dirname, '../../../tests/fixtures/ft8-210703-133430.wav')}`,
  ] });
  try {
    const page = await browser.newPage({ viewport: { width: 1200, height: 1000 } });
    const errors = [];
    page.on('pageerror', error => errors.push(error.message));
    await page.addInitScript(() => {
      window.testStreams = [];
      window.testEvents = [];
      const OriginalWorker = window.Worker;
      window.Worker = class extends OriginalWorker {
        constructor(...args) {
          super(...args);
          window.testWorker = this;
          this.addEventListener('message', event => window.testEvents.push(event.data));
        }
      };
      const getUserMedia = navigator.mediaDevices.getUserMedia.bind(navigator.mediaDevices);
      navigator.mediaDevices.getUserMedia = async constraints => {
        const stream = await getUserMedia(constraints);
        window.testStreams.push(stream);
        return stream;
      };
    });
    await page.goto(process.argv[2] || 'http://127.0.0.1:8082');
    const start = page.getByRole('button', { name: 'Start listening', exact: true });
    const stop = page.getByRole('button', { name: 'Stop', exact: true });
    await start.click();
    await page.getByRole('status').filter({ hasText: 'Listening for FT8' }).waitFor({ timeout: 35000 });
    // Exercise the actual AudioWorklet -> worker -> decoder -> UI path first.
    await page.locator('.conversation').first().waitFor({ timeout: 60000 });
    assert.ok(await page.locator('tbody tr').count() > 0);
    assert.ok(await stop.isEnabled());
    console.log('Live fixture produced conversation cards and raw messages before Stop');
    await stop.click();
    await page.getByRole('status').filter({ hasText: 'Final FT8 decode complete' }).waitFor({ timeout: 60000 });
    assert.ok(await page.evaluate(() => window.testStreams.every(s => s.getTracks().every(t => t.readyState === 'ended'))));
    const frozen = await page.locator('.conversation-list').innerText();
    await page.waitForTimeout(1100);
    assert.equal(await page.locator('.conversation-list').innerText(), frozen);
    await start.click();
    await page.getByRole('status').filter({ hasText: 'Listening for FT8' }).waitFor({ timeout: 35000 });
    assert.equal(await page.locator('.conversation').count(), 0, 'Start clears prior conversations');

    // Send controlled worker envelopes to check lifecycle without minutes of silence.
    const epoch = Date.now() + 600000;
    async function send(seconds, messages = []) {
      await page.evaluate(({ epoch, seconds, messages }) => {
        window.testWorker.dispatchEvent(new MessageEvent('message', { data: {
          kind: 'update', update: {
            capture_start_ms: epoch, capture_end_ms: epoch + seconds * 1000,
            received_seconds: seconds, next_attempt_utc_ms: epoch + (seconds + 15) * 1000,
            decodes: messages.map(([slot, message]) => ({
              start_utc_ms: epoch + slot * 15000, end_utc_ms: epoch + slot * 15000 + 12640,
              frequency_hz: 1200, snr_db: -12, message, payload: '0'.repeat(74) + '001',
            })),
          },
        } }));
      }, { epoch, seconds, messages });
      await page.waitForTimeout(100);
    }
    async function tileColor(locator) {
      return locator.evaluate(element => getComputedStyle(element).borderTopColor);
    }
    async function layout(locator) {
      return locator.evaluate(element => {
        const rect = element.getBoundingClientRect();
        return { x: rect.x + scrollX, y: rect.y + scrollY, width: rect.width, height: rect.height };
      });
    }
    await send(13, [[0, 'K1ABC W9XYZ R-12']]);
    let card = page.locator('.conversation').filter({ hasText: 'W9XYZ ↔ K1ABC' });
    assert.equal(await card.locator('.exchange-slot').count(), 6);
    assert.equal(await card.locator('.exchange-slot.missing').count(), 3);
    const rogerColor = await tileColor(card);
    assert.match(await card.locator('.exchange-slot.current').innerText(), /R-report/);
    assert.match(await card.locator('.exchange-slot.expected').getAttribute('aria-label'), /Acknowledgment/);
    await card.hover();
    const tooltip = page.locator('#conversation-tooltip');
    await tooltip.waitFor();
    assert.match(await tooltip.locator('.inference').innerText(), /K1ABC.*not heard transmitting/);
    assert.match(await tooltip.locator('.conversation-evidence').innerText(), /K1ABC W9XYZ R-12/);
    await page.mouse.move(0, 0);
    await card.focus();
    await tooltip.waitFor();
    await send(14);
    assert.equal(await card.evaluate(element => element === document.activeElement), true);
    await page.keyboard.press('Escape');
    await tooltip.waitFor({ state: 'hidden' });
    assert.doesNotMatch(await card.innerText(), /UTC|received messages|Next expected/);
    await send(73);
    assert.match(await card.locator('.conversation-state').innerText(), /Stale/);
    const staleColor = await tileColor(card);
    assert.notEqual(staleColor, rogerColor, 'stale overrides the last received stage color');
    await send(118);
    assert.equal(await page.locator('.conversation').count(), 0);
    await send(133, [[8, 'CQ K1ABC FN42']]);
    const callingColor = await tileColor(page.locator('.conversation'));
    await send(148, [[9, 'K1ABC W9XYZ EN37']]);
    card = page.locator('.conversation').filter({ hasText: 'K1ABC ↔ W9XYZ' });
    const firstPosition = await layout(card);
    const replyColor = await tileColor(card);
    await send(149, [[9, 'CQ K7NEW EM00'], ...Array.from({ length: 30 }, (_, i) => [9, [`CQ W${i}XYZ EM00`, `K9ABC W${i}XYZ EM00`, `K9ABC W${i}XYZ -12`, `K9ABC W${i}XYZ R-12`, `K9ABC W${i}XYZ RRR`][i % 5]])]);
    const neighbor = page.locator('.conversation').filter({ hasText: 'K7NEW ↔ CQ' });
    const neighborPosition = await layout(neighbor);
    assert.deepEqual(await layout(card), firstPosition, 'new tiles do not resize existing tiles');
    assert.ok(neighborPosition.x > firstPosition.x && neighborPosition.y === firstPosition.y, 'tiles form columns');
    assert.equal(await page.locator('.conversation').count(), 32);
    assert.ok((await layout(page.locator('.conversation-list'))).height < 820, '32 conversations fit in a compact board');
    assert.ok(firstPosition.height <= 195 && firstPosition.width < 180, 'tiles remain very small');
    await send(163, [[10, 'W9XYZ K1ABC -12']]);
    const reportColor = await tileColor(card);
    assert.equal(await card.locator('.exchange-slot.received').nth(1).locator('.exchange-message').innerText(), 'K1ABC W9XYZ EN37');
    assert.equal(await card.locator('.exchange-slot.received').nth(2).locator('.exchange-message').innerText(), 'W9XYZ K1ABC -12');
    await send(178, [[11, 'K1ABC W9XYZ R-10']]);
    assert.equal(await tileColor(card), rogerColor, 'R-report colors agree across contacts');
    assert.equal(await card.locator('.exchange-slot.current .exchange-message').innerText(), 'K1ABC W9XYZ R-10');
    await send(193, [[12, 'W9XYZ K1ABC RR73']]);
    assert.match(await card.locator('.conversation-state').innerText(), /Ended/);
    const endedColor = await tileColor(card);
    const acknowledgmentColor = await tileColor(page.locator('.conversation').filter({ hasText: 'W4XYZ ↔ K9ABC' }));
    assert.equal(new Set([callingColor, replyColor, reportColor, rogerColor, acknowledgmentColor, endedColor, staleColor]).size, 7, 'all seven conversation states have distinct colors');
    assert.equal(await card.locator('.exchange-slot.received').count(), 5);
    assert.match(await card.locator('.exchange-slot.current').innerText(), /RRR \/ RR73/);
    assert.equal(await card.locator('.exchange-slot.current .exchange-message').innerText(), 'W9XYZ K1ABC RR73');
    assert.equal(await card.locator('.exchange-slot.missing .exchange-message').count(), 0);
    assert.equal(await card.locator('.exchange-slot.optional').count(), 1);
    assert.deepEqual(await layout(card), firstPosition, 'progress does not move or resize tiles');
    assert.deepEqual(await layout(neighbor), neighborPosition, 'neighbors stay fixed during progress');
    const stages = await card.locator('.exchange-slot').evaluateAll(elements => elements.map(e => ({ x: e.getBoundingClientRect().x, y: e.getBoundingClientRect().y })));
    assert.ok(stages.every((p, i) => p.x === stages[0].x && (i === 0 || p.y > stages[i - 1].y)), 'stages stack vertically');
    await page.mouse.move(0, 0);
    if (process.env.WSJT_BROWSER_SCREENSHOT) {
      await page.locator('section[aria-label="FT8 conversations"]').screenshot({ path: process.env.WSJT_BROWSER_SCREENSHOT });
    }
    await card.focus();
    await tooltip.waitFor();
    assert.match(await tooltip.innerText(), /RR73 heard/);
    assert.equal(await tooltip.locator('.conversation-evidence li').count(), 5);
    assert.deepEqual(await layout(neighbor), neighborPosition, 'hover details do not change tile geometry');
    await page.keyboard.press('Escape');
    await page.setViewportSize({ width: 375, height: 812 });
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
    assert.ok(await card.locator('.exchange-slots').evaluate(element => element.scrollWidth <= element.clientWidth));
    if (process.env.WSJT_BROWSER_SCREENSHOT) {
      await page.locator('section[aria-label="FT8 conversations"]').screenshot({ path: process.env.WSJT_BROWSER_SCREENSHOT.replace(/\.png$/, '-mobile.png') });
    }
    await page.setViewportSize({ width: 2400, height: 1200 });
    assert.ok((await layout(page.locator('.conversation-list'))).width > 2300, 'board uses the full browser width');
    await page.setViewportSize({ width: 1200, height: 1000 });
    await send(238);
    assert.equal(await page.locator('.conversation').count(), 31);
    assert.deepEqual(await layout(neighbor), neighborPosition, 'expiry preserves grid vacancies');
    await send(239, [[15, 'CQ N4NEW EM00']]);
    assert.deepEqual(await layout(neighbor), neighborPosition, 'vacancy reuse does not shift survivors');
    const replacement = page.locator('.conversation').filter({ hasText: 'N4NEW ↔ CQ' });
    assert.deepEqual(await layout(replacement), firstPosition);
    await send(343);
    assert.equal(await page.locator('.conversation').count(), 0);
    await stop.click();
    await page.getByRole('status').filter({ hasText: 'Final FT8 decode complete' }).waitFor({ timeout: 60000 });
    assert.deepEqual(errors, []);
    console.log('PASS: visible callsigns/locators/reports/R-reports/sign-off in stages, seven distinct state colors including stale and RR73, 32 compact tiles, vertical states, stable grid positions, hover/focus details, Escape dismissal, no dates, mobile layout, inference, expiry, Stop/restart and input release');
  } finally {
    await browser.close();
  }
})().catch(error => { console.error(error); process.exitCode = 1; });

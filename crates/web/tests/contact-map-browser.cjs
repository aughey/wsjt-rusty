// Optional release-browser regression. Run with NODE_PATH pointing at Playwright:
// node crates/web/tests/contact-map-browser.cjs http://127.0.0.1:8088
// Uses installed Chrome by default; WSJT_BROWSER_CHANNEL can select another channel.
const { chromium } = require('playwright');
const assert = require('node:assert/strict');
const path = require('node:path');
(async () => {
  const browser = await chromium.launch({channel: process.env.WSJT_BROWSER_CHANNEL || 'chrome', headless: true, args: [
    '--use-fake-device-for-media-stream', '--use-fake-ui-for-media-stream',
    `--use-file-for-fake-audio-capture=${path.resolve(__dirname, '../../../tests/fixtures/ft8-210703-133430.wav')}`,
  ]});
  try {
    const page = await browser.newPage({viewport: {width: 1200, height: 1000}});
    const errors = []; page.on('pageerror', e => errors.push(e.message));
    await page.addInitScript(() => {
      const OriginalWorker = Worker;
      window.Worker = class extends OriginalWorker {
        constructor(...args) { super(...args); window.testWorker = this; }
      };
      window.streams = [];
      const getUserMedia = navigator.mediaDevices.getUserMedia.bind(navigator.mediaDevices);
      navigator.mediaDevices.getUserMedia = async c => { const s = await getUserMedia(c); streams.push(s); return s; };
    });
    await page.goto(process.argv[2] || 'http://127.0.0.1:8088');
    const start = page.getByRole('button', {name: 'Start listening', exact: true});
    const stop = page.getByRole('button', {name: 'Stop', exact: true});
    await start.click();
    await page.locator('.conversation-list > .conversation').first().waitFor({timeout: 60000});
    assert.ok(await page.locator('tbody tr').count() > 0, 'independent fixture decodes live');
    await stop.click();
    await page.getByRole('status').filter({hasText: 'Final FT8 decode complete'}).waitFor({timeout: 60000});
    assert.ok(await page.evaluate(() => streams.every(s => s.getTracks().every(t => t.readyState === 'ended'))));
    await start.click();
    await page.getByRole('status').filter({hasText: 'Listening for FT8'}).waitFor({timeout: 35000});
    const epoch = Date.now() + 600000;
    async function send(messages) {
      await page.evaluate(({epoch, messages}) => {
        testWorker.dispatchEvent(new MessageEvent('message', {data: {kind: 'update', update: {
          capture_start_ms: epoch, capture_end_ms: epoch + 13000, received_seconds: 13, next_attempt_utc_ms: epoch + 15000,
          decodes: messages.map(([message, snr_db], i) => ({start_utc_ms: epoch, end_utc_ms: epoch + 12640, frequency_hz: 1000 + i*100,
            message, snr_db, payload: [...message].reduce((n,c) => Math.imul(n ^ c.charCodeAt(0), 16777619) >>> 0, 2166136261).toString(2).padStart(74, '0') + '001'})),
        }}}));
      }, {epoch, messages});
    }
    await page.getByLabel('My grid', {exact: true}).fill('EN37');
    await send([['CQ K1ABC FN42', -5], ['CQ N0XYZ EN37', -20], ['CQ W6XYZ CM87', 3], ['W9XYZ K7ABC -10', -12]]);
    const board = page.locator('.conversation-list');
    const cards = board.locator(':scope > .conversation');
    await cards.nth(3).waitFor();
    const order = () => cards.locator('h3').allTextContents();
    const conversationSort = page.locator('[aria-label="Sort conversations"]');
    await conversationSort.getByRole('button', {name: 'Distance ↑'}).click();
    assert.deepEqual(await order(), ['N0XYZ ↔ CQ', 'K1ABC ↔ CQ', 'W6XYZ ↔ CQ', 'K7ABC ↔ W9XYZ']);
    assert.equal(await page.locator('.card-travel').count(), 0, 'card movement animations are removed');
    await conversationSort.getByRole('button', {name: 'Signal ↓'}).click();
    assert.equal((await order())[0], 'W6XYZ ↔ CQ');
    await conversationSort.getByRole('button', {name: 'State · CQ first'}).click();
    assert.ok((await order())[0].includes('CQ'), 'CQ sorts before reports');
    const before = await order();
    await send([['CQ K4NEW EM48', 10]]);
    assert.deepEqual((await order()).slice(0, 4), before, 'later activity preserves explicit ordering');
    await cards.filter({hasText: 'K1ABC ↔ CQ'}).click();
    await cards.filter({hasText: 'W6XYZ ↔ CQ'}).click();
    const queue = page.locator('.station-queue');
    const stations = queue.locator(':scope > .queued-station');
    assert.deepEqual(await stations.evaluateAll(es => es.map(e => e.dataset.station)), ['W6XYZ', 'K1ABC']);
    await stations.filter({hasText: 'K1ABC'}).getByRole('button', {name: 'Move to front', exact: true}).click();
    assert.deepEqual(await stations.evaluateAll(es => es.map(e => e.dataset.station)), ['K1ABC', 'W6XYZ']);
    const queueSort = page.locator('[aria-label="Sort stations to contact"]');
    await queueSort.getByRole('button', {name: 'Signal ↓'}).click();
    assert.deepEqual(await stations.evaluateAll(es => es.map(e => e.dataset.station)), ['W6XYZ', 'K1ABC']);
    assert.match(await stations.filter({hasText: 'K1ABC'}).locator('.station-location').innerText(), /FN42.*42\.500.*-71\.000.*km/);
    await page.getByRole('button', {name: 'Open map', exact: true}).click();
    await page.locator('#contact-map .ol-viewport').waitFor();
    await page.getByText('Located stations (text list)', {exact: true}).click();
    const mapPanel = page.locator('.contact-map-panel');
    assert.match(await mapPanel.innerText(), /K1ABC.*FN42/);
    assert.match(await mapPanel.innerText(), /My station.*EN37/);
    assert.doesNotMatch(await mapPanel.locator('li').allTextContents().then(s => s.join('\n')), /K7ABC/);
    assert.ok(await mapPanel.getByRole('link', {name: 'OpenStreetMap', exact: true}).isVisible());
    await page.getByRole('button', {name: 'Close map', exact: true}).click();
    assert.equal(await page.locator('#contact-map .ol-viewport').count(), 0);
    await page.getByRole('button', {name: 'Open map', exact: true}).click();
    await page.locator('#contact-map .ol-viewport').waitFor();
    await page.setViewportSize({width: 375, height: 812});
    assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
    await stop.click();
    await page.getByRole('status').filter({hasText: 'Final FT8 decode complete'}).waitFor({timeout: 60000});
    assert.ok(await page.evaluate(() => streams.every(s => s.getTracks().every(t => t.readyState === 'ended'))));
    await start.click();
    await page.getByRole('status').filter({hasText: 'Listening for FT8'}).waitFor({timeout: 35000});
    assert.equal(await stations.count(), 2);
    assert.ok((await stations.locator('.station-location').allTextContents()).every(t => t.includes('unknown')));
    await stop.click();
    await page.getByRole('status').filter({hasText: 'Final FT8 decode complete'}).waitFor({timeout: 60000});
    await page.locator('section[aria-label="Stations to contact"]').getByRole('button',{name:'Remove all',exact:true}).click();
    assert.equal(await stations.count(),0);
    assert.deepEqual(errors, []);
    console.log('PASS: live fixture, Stop/restart/release, one-shot sorts, front priority, grid distance, no movement animations, Remove all, CQ-first state, local OpenLayers/OSM map lifecycle, mobile layout');
  } finally { await browser.close(); }
})().catch(e => {console.error(e); process.exitCode = 1;});

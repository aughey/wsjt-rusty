// NODE_PATH=target/browser-qa/node_modules node crates/web/tests/layout-browser.cjs URL
const { chromium } = require('playwright');
const assert = require('node:assert/strict');
const path = require('node:path');

(async () => {
  const browser = await chromium.launch({ headless: true });
  try {
    const page = await browser.newPage();
    await page.goto(process.argv[2] || 'http://127.0.0.1:8114');
    await page.locator('.station-queue').waitFor({state:'attached'});
    // Overflow stress fixture in the real shell, with the real station controls.
    await page.evaluate(() => {
      const queue = document.querySelector('.station-queue');
      for (let i = 0; i < 40; i++) {
        const card = document.createElement('article');
        card.className = 'queued-station';
        card.innerHTML = `<h3>Station ${i + 1}</h3>${'<p>Received exchange evidence and station details</p>'.repeat(16)}<button>Remove</button>`;
        queue.append(card);
      }
    });
    for (const size of [{width:3200,height:1562}, {width:1440,height:1000}, {width:1280,height:720}, {width:800,height:600}, {width:390,height:844}]) {
      await page.setViewportSize(size);
      const layout = await page.evaluate(() => {
        const pick = s => {
          const e = document.querySelector(s), r = e.getBoundingClientRect();
          return { top:r.top, bottom:r.bottom, height:r.height, width:r.width, scroll:e.scrollHeight, client:e.clientHeight, overflow:getComputedStyle(e).overflowY };
        };
        return {page:pick('.app-panel'), workspace:pick('.station-workspace'), plot:pick('.waterfall-plot'), canvas:pick('#waterfall'), panel:pick('.stations-panel'), queue:pick('.station-queue'), doc:document.documentElement.scrollHeight, wide:document.documentElement.scrollWidth, width:innerWidth, height:innerHeight};
      });
      assert.ok(layout.doc <= size.height + 1, `document overflow at ${JSON.stringify(size)}`);
      assert.ok(layout.wide <= size.width + 1, 'no horizontal page overflow');
      assert.ok(layout.page.scroll <= layout.page.client + 1, `outer panel must not scroll: ${JSON.stringify(layout)}`);
      assert.ok(layout.panel.scroll > layout.panel.client, 'station panel owns overflow');
      assert.equal(layout.queue.overflow, 'visible', 'cards must not nest a second scrollbar');
      assert.ok(layout.panel.bottom <= layout.page.bottom + 1, 'stations stay inside workspace');
      assert.ok(layout.plot.height > 30, `waterfall keeps usable height at ${JSON.stringify(size)}: ${JSON.stringify(layout)}`);
      assert.ok(Math.abs(layout.plot.height - layout.canvas.height) <= 3, 'canvas fills plot height');
      assert.ok(Math.abs(layout.plot.width - layout.canvas.width) <= 3, 'canvas fills plot width');
      await page.locator('.station-queue').focus();
      await page.keyboard.press('Control+End');
      await page.waitForFunction(() => {
        const p = document.querySelector('.stations-panel');
        return p.scrollTop + p.clientHeight >= p.scrollHeight - 2;
      });
      assert.equal(await page.locator('.app-panel').evaluate(e => e.scrollTop), 0, 'keyboard scroll stays in stations panel');
      if (size.width === 1440 || size.width === 390) {
        await page.locator('.stations-panel').evaluate(e => { e.scrollTop = 0; });
        await page.screenshot({path:path.resolve(__dirname, `../../../target/stations-layout-${size.width}.png`)});
      }
    }
    console.log('PASS: bounded workspace, full waterfall canvas, one station scrollbar and keyboard reachability at five viewport sizes');
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });

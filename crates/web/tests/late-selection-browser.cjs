// Release-WASM regression: final-boundary station selection preserves the partner reply turn.
// NODE_PATH=target/browser-qa/node_modules node this-file.cjs URL
const { chromium } = require("playwright"),
  assert = require("node:assert/strict"),
  path = require("node:path");
(async () => {
  const browser = await chromium.launch({
    channel: process.env.WSJT_BROWSER_CHANNEL || "chromium",
    headless: true,
    args: [
      "--use-fake-device-for-media-stream",
      "--use-fake-ui-for-media-stream",
      `--use-file-for-fake-audio-capture=${path.resolve(__dirname, "../../../tests/fixtures/ft8-210703-133430.wav")}`,
    ],
  });
  try {
    const page = await browser.newPage({
        viewport: { width: 1440, height: 1000 },
      }),
      errors = [];
    page.on("pageerror", (e) => errors.push(e.message));
    await page.addInitScript(() => {
      window.testNow = Date.parse("2026-11-23T01:30:43.000Z");
      Date.now = () => testNow;
      window.outputs = [];
      window.testWorkers = [];
      const NativeWorker = Worker;
      window.Worker = class extends NativeWorker {
        constructor(...args) {
          super(...args);
          testWorkers.push(this);
        }
        postMessage(data, ...rest) {
          if (data.role) this.role = data.role;
          return super.postMessage(data, ...rest);
        }
      };
      const NativeAudio = AudioContext;
      let count = 0;
      class Output {
        constructor() {
          this.origin = testNow;
          this.state = "running";
        }
        get currentTime() {
          return (testNow - this.origin) / 1000;
        }
        async resume() {
          this.state = "running";
        }
        async setSinkId(id) {
          this.sinkId = id;
        }
        createGain() {
          return {
            gain: {
              value: 1,
              cancelScheduledValues() {},
              setTargetAtTime() {},
            },
            connect() {},
          };
        }
        createBuffer(channels, length, rate) {
          return {
            pcm: new Float32Array(length),
            copyToChannel(data) {
              this.pcm.set(data);
            },
          };
        }
        createBufferSource() {
          const ctx = this;
          return {
            connect() {},
            disconnect() {},
            stop() {
              if (this.row) this.row.stopped = true;
            },
            start(when) {
              this.row = {
                when,
                context: ctx,
                source: this,
                pcm: this.buffer.pcm.slice(),
                played: false,
                ended: false,
                stopped: false,
              };
              outputs.push(this.row);
            },
          };
        }
        async close() {
          this.state = "closed";
        }
      }
      window.AudioContext = function (...args) {
        return ++count === 1 ? new NativeAudio(...args) : new Output();
      };
      window.AudioContext.prototype = NativeAudio.prototype;
      window.advanceClock = (ms) => {
        testNow = Date.parse("2026-11-23T00:00:00.000Z") + ms;
        for (const row of outputs) {
          if (!row.stopped && row.context.currentTime >= row.when)
            row.played = true;
          if (
            !row.stopped &&
            !row.ended &&
            row.context.currentTime >= row.when + 12.64
          ) {
            row.ended = true;
            row.source.onended?.();
          }
        }
      };
      window.emitDecode = (ms, message, snr = -10, frequency = 2621.4) => {
        let hash = 2166136261;
        for (const c of message)
          hash = Math.imul(hash ^ c.charCodeAt(0), 16777619) >>> 0;
        const payload = hash.toString(2).padStart(74, "0") + "001",
          start = Date.parse("2026-11-23T00:00:00.000Z") + ms;
        testWorkers
          .find((w) => w.role === "slot")
          .dispatchEvent(
            new MessageEvent("message", {
              data: {
                kind: "decode-found",
                decode: {
                  start_utc_ms: start,
                  end_utc_ms: start + 12640,
                  frequency_hz: frequency,
                  snr_db: snr,
                  message,
                  payload,
                },
              },
            }),
          );
      };
      window.completeSlot = (ms) => {
        const base = Date.parse("2026-11-23T00:00:00.000Z");
        testWorkers
          .find((w) => w.role === "slot")
          .dispatchEvent(
            new MessageEvent("message", {
              data: {
                kind: "update",
                update: {
                  capture_start_ms: base + 5413000,
                  capture_end_ms: base + ms + 14900,
                  received_seconds: 13.5,
                  next_attempt_utc_ms: base + ms + 29900,
                  completed_slot_utc_ms: base + ms,
                  decodes: [],
                },
              },
            }),
          );
      };
    });
    await page.goto(process.argv[2] || "http://127.0.0.1:8097");
    const select = async (name) =>
      page.getByRole("tab", { name, exact: true }).click();
    await select("Contact configuration");
    await page.getByLabel("My callsign", { exact: true }).fill("N9IOX");
    await page.getByLabel("My grid", { exact: true }).fill("EM48");
    await page.getByLabel("Use connected radio PTT").uncheck();
    await page.getByLabel("find an open frequency", { exact: true }).uncheck();
    await page
      .getByLabel("Transmit audio output", { exact: true })
      .selectOption("default");
    await page
      .getByRole("button", { name: "Start listening", exact: true })
      .click();
    await page
      .getByRole("status")
      .filter({ hasText: "Listening for FT8" })
      .waitFor({ timeout: 30000 });
    const tick = async (ms) => {
      await page.evaluate((ms) => advanceClock(ms), ms);
      await page.waitForTimeout(300);
    };
    const hear = async (ms, text, snr = -10) => {
      await page.evaluate(({ ms, text, snr }) => emitDecode(ms, text, snr), {
        ms,
        text,
        snr,
      });
      await page.waitForTimeout(30);
    };
    const done = async (ms) => page.evaluate((ms) => completeSlot(ms), ms),
      count = () => page.evaluate(() => outputs.length);
    const queue = async (call) => {
      await select("Conversations");
      await page
        .locator(".conversation-list > .conversation")
        .filter({ hasText: call })
        .first()
        .click();
      await select("Stations to contact");
    };



    await hear(5430500, "CQ VA3FAT FN06", -10);
    await queue("VA3FAT");
    await page.getByRole("button", {name:"Start contacting",exact:true}).click();
    await tick(5444799);
    assert.equal(await page.evaluate(()=>outputs.length),0,'no initial target before final selection window');
    await tick(5444900);
    await page.waitForFunction(()=>outputs.length===1);
    await tick(5445600);
    await tick(5458400);
    await hear(5445500,"CQ K1ABC FN42",-5);
    await queue("K1ABC");
    await tick(5459900);
    assert.equal(await page.evaluate(()=>outputs.length),1,'opposite-parity CQ cannot take the partner receive turn');
    assert.match(await page.locator('.active-conversation').innerText(),/VA3FAT/);
    await hear(5460500,"CQ K1ABC FN42",-5);
    await tick(5474799);
    assert.equal(await page.evaluate(()=>outputs.length),1,'no switch at the old early reservation time');
    assert.match(await page.locator('.active-conversation').innerText(),/VA3FAT/);
    await page.evaluate(()=>emitDecode(5460888,"N9IOX VA3FAT -04",-14.6,801.6));
    await tick(5474900);
    await page.waitForFunction(()=>outputs.length===2);
    assert.match(await page.locator('.contact-policy-decision').innerText(),/VA3FAT N9IOX R-15/);
    assert.match(await page.locator('.active-conversation').innerText(),/Active conversation: VA3FAT/);
    await tick(5475600);
    await tick(5488400);
    await hear(5490888,"N9IOX VA3FAT RR73",-14.6);
    await tick(5504900);
    await page.waitForFunction(()=>outputs.length===3);
    await tick(5505600);
    await tick(5518400);
    await select("Log");
    const row=page.locator('#log-contacts tbody tr').filter({hasText:'VA3FAT'}).filter({hasText:'Completed'});
    await row.waitFor();
    assert.equal(await page.locator('#log-contacts tbody tr').filter({hasText:'K1ABC'}).count(),0,'opposite station never becomes an attempt');
    await row.getByRole('button',{name:/View.*events/}).click();
    await page.locator('#log-evidence tbody tr').filter({hasText:'TX audio'}).filter({hasText:'VA3FAT N9IOX R-15'}).waitFor();
    assert.doesNotMatch(await page.locator('#log-evidence').innerText(),/Attempt replaced/);
    // A newer locator after a report rewinds the owed response. The queue's
    // furthest-heard display stage must not choose what the procedure sends.
    await select("Stations to contact");
    await hear(5520500,"CQ K1ABC FN42",-10);
    await tick(5534900);
    await page.waitForFunction(()=>outputs.length===4);
    await tick(5535600); await tick(5548400);
    await hear(5550500,"N9IOX K1ABC -04",-10);
    await tick(5564900);
    await page.waitForFunction(()=>outputs.length===5);
    assert.match(await page.locator('.contact-policy-decision').innerText(),/K1ABC N9IOX R-10/);
    await tick(5565600); await tick(5578400);
    await hear(5580500,"N9IOX K1ABC FN42",-10);
    await tick(5594900);
    await page.waitForFunction(()=>outputs.length===6);
    assert.match(await page.locator('.contact-policy-decision').innerText(),/K1ABC N9IOX -10/);
    await tick(5595600); await tick(5608400);
    await hear(5610500,"N9IOX K1ABC -06",-10);
    await tick(5624900);
    await page.waitForFunction(()=>outputs.length===7);
    assert.match(await page.locator('.contact-policy-decision').innerText(),/K1ABC N9IOX R-10/);
    await tick(5625600); await tick(5638400);
    await hear(5640500,"N9IOX K1ABC RR73",-10);
    await tick(5654900);
    await page.waitForFunction(()=>outputs.length===8);
    await tick(5655600); await tick(5668400);
    await select("Log");
    await page.getByRole('tab',{name:'Contacts',exact:true}).click();
    await page.locator('#log-contacts tbody tr').filter({hasText:'K1ABC'}).filter({hasText:'Completed'}).waitFor();
    await page.getByRole('button',{name:'Stop',exact:true}).click();
    await page.getByRole('status').filter({hasText:'Final FT8 decode complete'}).waitFor({timeout:60000});
    assert.deepEqual(errors,[]);
    console.log('PASS: final-boundary selection preserves the receive turn; newer locator rewinds the response and the exchange completes');
  } finally {
    await browser.close();
  }
})().catch((e) => {
  console.error(e);
  process.exitCode = 1;
});

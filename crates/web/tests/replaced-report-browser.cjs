// Release-WASM regression: fresh partner report reclaims a CQ-replaced attempt.
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


    await hear(5430500, "CQ QRP KM4FZA EM95", -10.5);
    await queue("KM4FZA");
    await page.getByRole("button",{name:"Start contacting",exact:true}).click();
    await tick(5444900);
    await page.waitForFunction(()=>outputs.length===1);
    await tick(5445600);
    await tick(5459900);
    await hear(5460500,"CQ N0XYZ EM00");
    await queue("N0XYZ");
    await tick(5474900);
    await page.waitForFunction(()=>outputs.length===2);
    await tick(5475600);
    await tick(5489900);
    assert.match(await page.locator('.queued-station[data-station="KM4FZA"]').innerText(),/Attempt replaced by fresh CQ/);
    await select("Log");
    const records=page.locator('#log-contacts tbody tr').filter({hasText:'KM4FZA'});
    await records.filter({hasText:'Attempt replaced by fresh CQ'}).waitFor();
    const originalStart=await records.locator('td').first().innerText();
    await select("Stations to contact");
    await hear(5490500,"N9IOX KM4FZA +00",-5.2);
    await tick(5504900);
    await page.waitForFunction(()=>outputs.length===3);
    assert.match(await page.locator('.active-conversation').innerText(),/Active conversation: KM4FZA/);
    assert.match(await page.locator('.contact-policy-decision').innerText(),/KM4FZA N9IOX R-05/);
    await tick(5505600);
    await tick(5519900);
    await hear(5520500,"N9IOX KM4FZA RR73",-5.2);
    await tick(5534900);
    await page.waitForFunction(()=>outputs.length===4);
    await tick(5535600);
    await tick(5549900);
    await select("Log");
    await records.filter({hasText:'Completed'}).waitFor();
    assert.equal(await records.count(),1,'resumed exchange has a single contact record');
    assert.equal(await records.locator('td').first().innerText(),originalStart);
    await records.getByRole('button',{name:/View.*events/}).click();
    const evidence=page.locator('#log-evidence tbody tr');
    await evidence.filter({hasText:'N9IOX KM4FZA +00'}).waitFor();
    assert.equal(await evidence.filter({hasText:'TX audio'}).filter({hasText:'KM4FZA N9IOX R-05'}).count(),1);
    assert.equal(await evidence.filter({hasText:'TX audio'}).filter({hasText:'KM4FZA N9IOX EM48'}).count(),1);
    const row=evidence.filter({hasText:'N9IOX KM4FZA +00'});
    assert.notEqual(await row.locator('td').nth(2).innerText(),'','decode-arrival UTC is retained');
    await page.getByRole('button',{name:'Stop',exact:true}).click();
    await page.getByRole('status').filter({hasText:'Final FT8 decode complete'}).waitFor({timeout:60000});
    await page.reload();
    await select("Log");
    await records.filter({hasText:'Completed'}).waitFor();
    assert.equal(await records.count(),1);
    await records.getByRole('button',{name:/View.*events/}).click();
    await evidence.filter({hasText:'N9IOX KM4FZA +00'}).waitFor();
    assert.deepEqual(errors,[]);
    console.log('PASS: KM4FZA report reclaims CQ-replaced call, plays R-report, completes under original record, and persists evidence after reload');
  } finally {
    await browser.close();
  }
})().catch((e) => {
  console.error(e);
  process.exitCode = 1;
});

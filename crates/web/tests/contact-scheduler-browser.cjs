// Release-WASM UI/transport regression: delayed decode delivery and self echoes.
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
    // Wait for the WASM shell before querying roles under the frozen test clock.
    await page.locator(".app-tabs button").first().waitFor();
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
    await hear(5430827, "CQ W4VPK FM02", -7.7);
    await queue("W4VPK");
    await page
      .getByRole("button", { name: "Start contacting", exact: true })
      .click();
    await tick(5444900);
    await page.waitForFunction(() => outputs.length === 1);
    assert.match(await page.locator('.contact-policy-decision').innerText(), /Transmit next.*W4VPK N9IOX EM48/);
    assert.equal(await page.locator('.active-conversation').innerText(), 'Attempting contact: W4VPK');
    const rxMarker = page.locator('.rx-frequency-marker');
    assert.match(await rxMarker.getAttribute('aria-label'), /Attempting contact with W4VPK; latest received frequency 2621.4 audio Hz/);
    assert.equal(await rxMarker.locator('span').innerText(), 'RX W4VPK · 2621.4 Hz');
    // Same-frequency TX/RX labels remain distinct, including on a narrow screen.
    await page.setViewportSize({ width: 390, height: 844 });
    assert.equal(await rxMarker.locator('span').evaluate(el => {
      const label = el.getBoundingClientRect(), plot = el.closest('.waterfall-plot').getBoundingClientRect();
      const tx = document.querySelector('.tx-frequency-marker span').getBoundingClientRect();
      return label.left >= plot.left && label.right <= plot.right && label.top >= tx.bottom;
    }), true, 'RX label stays inside mobile plot below TX label');
    await page.screenshot({ path: path.resolve('target/active-contact-mobile.png'), fullPage: true });
    await page.setViewportSize({ width: 1440, height: 1000 });
    await page.screenshot({ path: path.resolve('target/active-contact-desktop.png'), fullPage: true });
    assert.equal(await page.locator('.queued-station.active .station-role').innerText(), 'Contact attempt');
    const history = page.locator('.queued-station.active details');
    await history.getByText('Received conversation history', { exact: true }).click();
    assert.match(await history.innerText(), /CQ W4VPK FM02/);
    await tick(5445600);
    await hear(5445592, "W4VPK N9IOX EM48", -5.1);
    await tick(5459900);
    await tick(5474900);
    assert.equal(
      await count(),
      1,
      "unfinished RX slot must not schedule another locator",
    );
    assert.match(await page.locator('.contact-policy-decision').innerText(), /Listening.*receive slot still decoding/);
    await page.evaluate(() => emitDecode(5460822, "N9IOX W4VPK -14", -15.7, 2675));
    await tick(5474900);
    await page.waitForFunction(() => outputs.length === 2);
    assert.equal(await page.locator('.active-conversation').innerText(), 'Active conversation: W4VPK');
    assert.match(await rxMarker.getAttribute('aria-label'), /In QSO with W4VPK; latest received frequency 2675.0 audio Hz/);
    assert.match(await page.locator('.tx-frequency-marker').getAttribute('aria-label'), /2675.0 audio Hz/, 'normal TX placement continues to follow the station when open-frequency selection is off');
    assert.equal(await page.locator('.queued-station.active .station-role').innerText(), 'Active conversation');
    assert.match(await page.locator('.contact-policy-decision').innerText(), /Transmit next.*W4VPK N9IOX R-16/);
    await tick(5475600);
    await hear(5475562, "W4VPK N9IOX R-16", -3.8);
    await tick(5489900);
    await tick(5504900);
    assert.equal(
      await count(),
      3,
      "established QSO repeats its R-report even without receive-slot completion",
    );
    await tick(5504900);
    await page.waitForFunction(() => outputs.length === 3);
    await tick(5505600);
    await hear(5505562, "W4VPK N9IOX R-16", -4.2);
    await tick(5519900);
    await hear(5520822, "N9IOX W4VPK RR73", -13.2);
    await tick(5534900);
    await page.waitForFunction(() => outputs.length === 4);
    await tick(5535600);
    await tick(5549900);
    await page.waitForFunction(
      () => !document.querySelector('.queued-station[data-station="W4VPK"]'),
    );
    assert.equal(await rxMarker.count(), 0, 'completion clears the active station band');
    // Recovery delivers a report just 50 ms before an already reserved retry,
    // between interval ticks. The stale locator must never actually play.
    await tick(5564900);
    await hear(5550827, "CQ K1ABC FN42");
    await queue("K1ABC");
    await page
      .getByRole("button", { name: "Start contacting", exact: true })
      .click();
    await tick(5564900);
    await page.waitForFunction(() => outputs.length === 5);
    await tick(5565600);
    await tick(5579900);
    await tick(5594900);
    await done(5580000);
    await tick(5594900);
    await page.waitForFunction(() => outputs.length === 6);
    await page.evaluate(async () => {
      advanceClock(5595450);
      emitDecode(5580822, "N9IOX K1ABC -12", -11);
      for (let i = 0; i < 10; i++) await Promise.resolve();
      if (!outputs[5].stopped)
        throw new Error(
          "Stale waveform survived immediate RX reaction before any interval tick",
        );
      advanceClock(5595600);
    });
    await page.waitForTimeout(300);
    assert.equal(
      await page.evaluate(() => outputs[5].played),
      false,
      "superseded locator never reaches playback",
    );
    assert.equal(
      await page
        .getByRole("button", { name: "Stop contacting", exact: true })
        .count(),
      1,
      "superseding must not disarm",
    );
    await done(5610000);
    await tick(5624900);
    await page.waitForFunction(() => outputs.length === 7);
    await tick(5625600);
    await tick(5639900);
    await hear(5640822, "N9IOX K1ABC RR73");
    await tick(5654900);
    await page.waitForFunction(() => outputs.length === 8);
    await tick(5655600);
    await tick(5669900);
    // An unanswered initial call is an attempt, so another fresh CQ can win
    // queue priority before a partner report establishes an exclusive QSO.
    await hear(5670827, "CQ KM4ESU EM55");
    await queue("KM4ESU");
    const startAgain = page.getByRole("button", { name: "Start contacting", exact: true });
    if (await startAgain.count()) await startAgain.click();
    await tick(5684900);
    await page.waitForFunction(() => outputs.length === 9);
    assert.equal(await page.locator('.active-conversation').innerText(), 'Attempting contact: KM4ESU');
    await tick(5685600);
    await tick(5699900);
    await hear(5700827, "CQ N5IH EM20");
    await queue("N5IH");
    await tick(5714900);
    await page.waitForFunction(() => outputs.length === 10);
    assert.equal(await page.locator('.active-conversation').innerText(), 'Attempting contact: N5IH');
    assert.match(await page.locator('.queued-station[data-station="KM4ESU"]').innerText(), /Attempt replaced by fresh CQ/);
    await tick(5715600);
    await tick(5729900);
    await page.getByRole("button", { name: "Stop", exact: true }).click();
    await page
      .getByRole("status")
      .filter({ hasText: "Final FT8 decode complete" })
      .waitFor({ timeout: 60000 });
    // Decode played PCM with the real release-WASM receiver. This verifies audio
    // content rather than assuming a correct state label means a correct waveform.
    const messages = await page.evaluate(async () => {
      const result = [];
      for (const row of outputs.filter((o) => o.played)) {
        const samples = new Float32Array(16 * 12000);
        samples.set(row.pcm, 6000);
        const worker = new Worker("./decoder-worker.js", { type: "module" }),
          channel = new MessageChannel(),
          messages = [];
        await new Promise((resolve, reject) => {
          const timeout = setTimeout(
            () => reject(new Error("PCM decode timed out")),
            30000,
          );
          worker.onerror = (e) => reject(new Error(e.message));
          worker.onmessage = ({ data }) => {
            if (data.kind === "ready")
              channel.port1.postMessage({ kind: "samples", frame: 0, samples });
            if (data.kind === "decode-found")
              messages.push(data.decode.message);
            if (data.kind === "update") {
              messages.push(...data.update.decodes.map((d) => d.message));
              clearTimeout(timeout);
              resolve();
            }
            if (data.kind === "error") reject(new Error(data.error));
          };
          worker.postMessage(
            { role: "slot", rate: 12000, epoch: 0, port: channel.port2 },
            [channel.port2],
          );
        });
        worker.terminate();
        channel.port1.close();
        result.push([...new Set(messages)]);
      }
      return result;
    });
    assert.deepEqual(
      messages.map((m) => m[0]),
      [
        "W4VPK N9IOX EM48",
        "W4VPK N9IOX R-16",
        "W4VPK N9IOX R-16",
        "W4VPK N9IOX 73",
        "K1ABC N9IOX EM48",
        "K1ABC N9IOX R-11",
        "K1ABC N9IOX 73",
        "KM4ESU N9IOX EM48",
        "N5IH N9IOX EM48",
      ],
    );
    assert.deepEqual(errors, []);
    console.log(
      "PASS: delayed W4VPK trace, self echoes, receive readiness, last-50ms cancellation, established QSO priority, fresh-CQ attempt replacement, and decoded playback PCM",
    );
  } finally {
    await browser.close();
  }
})().catch((e) => {
  console.error(e);
  process.exitCode = 1;
});

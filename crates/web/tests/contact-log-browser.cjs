// Optional end-to-end log check using real browser capture/output with fake devices.
// NODE_PATH=<Playwright modules> node .../contact-log-browser.cjs http://127.0.0.1:8088
const {chromium}=require('playwright');
const assert=require('node:assert/strict');
const path=require('node:path');
const fs=require('node:fs');
(async()=>{
    const browser=await chromium.launch({channel:'chrome',headless:true,args:['--use-fake-device-for-media-stream','--use-fake-ui-for-media-stream',`--use-file-for-fake-audio-capture=${path.resolve(__dirname,'../../../tests/fixtures/ft8-210703-133430.wav')}`]});
    try {
        const page=await browser.newPage({viewport:{width:1200,height:1000},acceptDownloads:true});
        const errors=[];page.on('pageerror',e=>errors.push(e.message));
        await page.addInitScript(()=>{
            const OriginalWorker=Worker;
            window.Worker=class extends OriginalWorker {constructor(...args){super(...args);if(String(args[0]).includes('decoder-worker'))window.decodeWorker=this;}};
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
        const url=process.argv[2]||'http://127.0.0.1:8088';
        await page.goto(url);
        const log=page.locator('.contact-log'),panel=page.locator('[aria-label="Stations to contact"]');
        await log.getByRole('status').filter({hasText:'Stored on this browser'}).waitFor();
        const radio=page.locator('.radio-panel');
        await radio.getByLabel('Dial frequency (Hz)',{exact:true}).fill('21074000');
        await radio.getByRole('button',{name:'Connect radio',exact:true}).click();
        await radio.locator('.radio-dial').filter({hasText:'14.074000 MHz'}).waitFor();
        await panel.getByLabel('My callsign',{exact:true}).fill('W9XYZ');
        await panel.getByLabel('My grid',{exact:true}).fill('EN37');
        await panel.getByLabel('Use connected radio PTT').uncheck();
        await panel.getByLabel('Transmit audio output',{exact:true}).selectOption('default');
        await page.getByRole('button',{name:'Start listening',exact:true}).click();
        await page.getByRole('status').filter({hasText:'Listening for FT8'}).waitFor({timeout:35000});
        async function send(ms,message) {
            await page.evaluate(({ms,message})=>{
                const payload=[...message].reduce((n,c)=>Math.imul(n^c.charCodeAt(0),16777619)>>>0,2166136261).toString(2).padStart(74,'0')+'001';
                decodeWorker.dispatchEvent(new MessageEvent('message',{data:{kind:'update',update:{capture_start_ms:ms,capture_end_ms:ms+12640,received_seconds:13,next_attempt_utc_ms:ms+15000,
                    decodes:[{start_utc_ms:ms,end_utc_ms:ms+12640,message,payload,frequency_hz:1200,snr_db:-10}]}}}));
            },{ms,message});
        }
        const now=Date.now(),base=Math.floor(now/15000)*15000+(now%15000>12500?15000:0)+500;
        await send(base,'CQ K1ABC FN42');
        await page.locator('.conversation-list > .conversation').filter({hasText:'K1ABC ↔ CQ'}).click();
        await panel.getByRole('button',{name:'Start contacting',exact:true}).click();
        const station=panel.locator('[data-station="K1ABC"]');
        await station.locator('.transmit-steps .sent').waitFor({timeout:50000});
        await log.locator('#log-contacts tbody tr').filter({hasText:'K1ABC'}).waitFor();
        await send(base+30000,'W9XYZ K1ABC 73');
        await station.waitFor({state:'detached'});
        await log.locator('#log-contacts tbody tr').filter({hasText:'Completed'}).waitFor();
        await log.getByRole('button',{name:'Download ADIF',exact:true}).waitFor();
        await page.waitForFunction(()=>!document.querySelector('.contact-log button:not(.secondary):not([role="tab"])')?.disabled);
        const contacted=page.locator('.conversation-list > .conversation.contacted').filter({hasText:'K1ABC'});
        await contacted.waitFor();assert.match(await contacted.innerText(),/Contacted/);
        await log.getByRole('button',{name:/View \d+ events/}).first().click();
        await log.getByRole('tabpanel',{name:'Evidence',exact:true}).waitFor();
        assert.match(await log.locator('#log-evidence').innerText(),/TX audio/);
        assert.match(await log.locator('#log-evidence').innerText(),/W9XYZ K1ABC 73/);
        assert.match(await log.locator('#log-evidence').innerText(),/\d{4}-\d{2}-\d{2}T/);
        await log.getByRole('tab',{name:'Evidence',exact:true}).press('ArrowLeft');
        assert.equal(await log.getByRole('tab',{name:'Contacts',exact:true}).getAttribute('aria-selected'),'true');
        const download=page.waitForEvent('download');
        await log.getByRole('button',{name:'Download ADIF',exact:true}).click();
        const file=await download,text=fs.readFileSync(await file.path(),'utf8');
        assert.match(text,/<CALL:5>K1ABC/);assert.match(text,/APP_WSJT_RUSTY_EVIDENCE/);assert.match(text,/TX audio/);assert.match(text,/<FREQ:9>14.074000/);assert.doesNotMatch(text,/21.074000/);assert.ok(await page.evaluate(()=>rig.writes.some(b=>b[4]===3)));
        await page.getByRole('button',{name:'Open map',exact:true}).click();
        await page.getByText('Communication paths (text list)',{exact:true}).click();
        assert.match(await page.locator('.contact-map-panel').innerText(),/1 communication paths/);
        if(process.env.WSJT_BROWSER_SCREENSHOT) await log.screenshot({path:process.env.WSJT_BROWSER_SCREENSHOT});
        await page.getByRole('button',{name:'Stop',exact:true}).click();
        await page.getByRole('status').filter({hasText:'Final FT8 decode complete'}).waitFor({timeout:60000});
        await page.reload();
        await log.locator('#log-contacts tbody tr').filter({hasText:'Completed'}).waitFor();
        await page.getByRole('button',{name:'Start listening',exact:true}).click();
        await page.getByRole('status').filter({hasText:'Listening for FT8'}).waitFor({timeout:35000});
        await send(Date.now()+60000,'CQ K1ABC FN42');
        await contacted.waitFor();
        await page.setViewportSize({width:375,height:812});
        assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth));
        await page.getByRole('button',{name:'Stop',exact:true}).click();
        await page.getByRole('status').filter({hasText:'Final FT8 decode complete'}).waitFor({timeout:60000});
        assert.deepEqual(errors,[]);
        console.log('PASS: successful local audio and received closing evidence saved in SQLite; automatic removal, contacted hatching, queried RF readback, map path, tab keys, ADIF download, reload persistence, mobile log');
    } finally {await browser.close();}
})().catch(e=>{console.error(e);process.exitCode=1;});

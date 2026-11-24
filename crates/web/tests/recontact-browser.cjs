// Run against an isolated release Trunk build. Requires Playwright on NODE_PATH.
const {chromium}=require('playwright');
const assert=require('node:assert/strict');
const path=require('node:path');
(async()=>{
  const browser=await chromium.launch({channel:'chrome',headless:true,args:[
    '--use-fake-device-for-media-stream','--use-fake-ui-for-media-stream',
    `--use-file-for-fake-audio-capture=${path.resolve(__dirname,'../../../tests/fixtures/ft8-210703-133430.wav')}`,
  ]});
  try {
    const page=await browser.newPage({viewport:{width:1200,height:1000}}),errors=[];
    page.on('pageerror',e=>errors.push(e.message));
    await page.addInitScript(()=>{
      window.testStreams=[];
      const OriginalWorker=Worker;
      window.Worker=class extends OriginalWorker {
        constructor(...args) {super(...args);if(String(args[0]).includes('decoder-worker'))window.decodeWorker=this;}
      };
      const media=navigator.mediaDevices.getUserMedia.bind(navigator.mediaDevices);
      navigator.mediaDevices.getUserMedia=async c=>{const stream=await media(c);testStreams.push(stream);return stream;};
    });
    await page.goto(process.argv[2]||'http://127.0.0.1:8096');
    await page.evaluate(async()=>{
      const now=Date.now(),day=86400000;
      const records=[['old','K1ABC',8,'Completed'],['recent','W1XYZ',1,'Completed'],['failed','N0XYZ',0,'Interrupted']].map(([id,target,age,outcome])=>({
        id,revision:1,target,start_ms:now-age*day-60000,end_ms:now-age*day,outcome,
        station_call:'W9XYZ',station_grid:'EN37',grid:'FN42',evidence:[],
      }));
      const w=new Worker('contact-log-worker.js');
      try { await new Promise((resolve,reject)=>{w.onmessage=({data})=>data.error?reject(new Error(data.error)):resolve(data.result);w.postMessage({id:1,action:'save',records});}); }
      finally {w.terminate();}
    });
    await page.reload();
    const tabs=page.getByRole('tablist',{name:'Application views',exact:true});
    const select=name=>tabs.getByRole('tab',{name,exact:true}).click();
    await select('Conversations');
    const bulk=page.getByRole('button',{name:'Add all CQ stations',exact:true});
    await bulk.waitFor();await page.waitForFunction(()=>!Array.from(document.querySelectorAll('button')).find(b=>b.textContent==='Add all CQ stations').disabled);
    assert.equal(await page.locator('#contact-exclusion').inputValue(),'always');
    const start=async()=>{
      await page.getByRole('button',{name:'Start listening',exact:true}).click();
      await page.getByRole('status').filter({hasText:'Listening for FT8'}).waitFor({timeout:35000});
    };
    const send=async()=>page.evaluate(()=>{
      const ms=Date.now();
      const decodes=['CQ K1ABC FN42','CQ W1XYZ FN42','CQ N0XYZ EN37'].map((message,i)=>({
        start_utc_ms:ms,end_utc_ms:ms+12640,message,payload:i.toString(2).padStart(74,'0')+'001',frequency_hz:1200+i*100,snr_db:-10,
      }));
      decodeWorker.dispatchEvent(new MessageEvent('message',{data:{kind:'update',update:{capture_start_ms:ms,capture_end_ms:ms+12640,received_seconds:13,next_attempt_utc_ms:ms+15000,decodes}}}));
    });
    const card=call=>page.locator('.conversation-list > .conversation').filter({hasText:call});
    await start();await send();await card('K1ABC').waitFor();
    assert.ok((await card('K1ABC').getAttribute('class')).includes('contacted'));
    assert.ok((await card('W1XYZ').getAttribute('class')).includes('contacted'));
    assert.ok(!(await card('N0XYZ').getAttribute('class')).includes('contacted'));
    await page.locator('#contact-exclusion').selectOption('days');
    assert.equal(await page.locator('#contact-exclusion-days').inputValue(),'7');
    await page.waitForFunction(()=>!Array.from(document.querySelectorAll('.conversation')).find(c=>c.textContent.includes('K1ABC')).classList.contains('contacted'));
    assert.ok((await card('W1XYZ').getAttribute('class')).includes('contacted'));
    await bulk.click();await select('Stations to contact');
    await page.locator('[data-station="K1ABC"]').waitFor();await page.locator('[data-station="N0XYZ"]').waitFor();
    assert.equal(await page.locator('[data-station="W1XYZ"]').count(),0);
    await page.getByRole('button',{name:'Stop',exact:true}).click();
    await page.getByRole('status').filter({hasText:'Final FT8 decode complete'}).waitFor({timeout:60000});
    assert.ok(await page.evaluate(()=>testStreams.every(s=>s.getTracks().every(t=>t.readyState==='ended'))));
    await page.reload();await select('Conversations');
    assert.equal(await page.locator('#contact-exclusion').inputValue(),'days');
    assert.equal(await page.locator('#contact-exclusion-days').inputValue(),'7');
    const auto=page.getByLabel('Automatically add new CQ stations');assert.equal(await auto.isChecked(),false);
    await auto.check();await start();await send();await select('Stations to contact');
    await page.locator('[data-station="K1ABC"]').waitFor();
    assert.equal(await page.locator('[data-station="W1XYZ"]').count(),0,'auto-add uses reloaded durable history and saved interval');
    await select('Conversations');await page.locator('#contact-exclusion').selectOption('off');
    await bulk.click();await select('Stations to contact');await page.locator('[data-station="W1XYZ"]').waitFor();
    await page.getByRole('button',{name:'Stop',exact:true}).click();
    await page.getByRole('status').filter({hasText:'Final FT8 decode complete'}).waitFor({timeout:60000});
    await select('Conversations');await page.setViewportSize({width:390,height:844});
    assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth),'mobile exclusion settings fit');
    await page.screenshot({path:path.resolve(__dirname,'../../../target/recontact-mobile.png'),fullPage:true});
    assert.deepEqual(errors,[]);
    console.log('PASS: durable log reload, saved 7-day interval, bulk/auto exclusion, off, incomplete attempts, Stop/restart, mobile');
  } finally {await browser.close();}
})().catch(error=>{console.error(error);process.exitCode=1;});

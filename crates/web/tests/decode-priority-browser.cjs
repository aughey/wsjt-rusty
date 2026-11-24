// Real release-WASM decoding of the independent FT8 fixture; no simulated decodes.
const { chromium } = require('playwright');
const fs = require('node:fs');
const path = require('node:path');
const assert = require('node:assert/strict');
(async () => {
  const browser = await chromium.launch({headless:true,args:['--no-sandbox']});
  try {
    const page = await browser.newPage();
    const errors=[]; page.on('pageerror',e=>errors.push(e.message));
    await page.goto(process.argv[2] || 'http://127.0.0.1:8083');
    const wav = fs.readFileSync(path.resolve(__dirname,'../../../tests/fixtures/ft8-210703-133430.wav')).toString('base64');
    const measurements = await page.evaluate(async wav => {
      const bytes=Uint8Array.from(atob(wav),c=>c.charCodeAt(0)), view=new DataView(bytes.buffer);
      let data, format;
      for(let p=12;p+8<=bytes.length;) {
        const name=String.fromCharCode(...bytes.slice(p,p+4)), size=view.getUint32(p+4,true);
        if(name==='fmt ') format={channels:view.getUint16(p+10,true),rate:view.getUint32(p+12,true),bits:view.getUint16(p+22,true)};
        if(name==='data') data={offset:p+8,size};
        p+=8+size+(size&1);
      }
      if(format.channels!==1 || format.rate!==12000 || format.bits!==16) throw new Error('Unexpected fixture WAV format');
      const pcm=new Float32Array(data.size/2); for(let i=0;i<pcm.length;i++) pcm[i]=view.getInt16(data.offset+i*2,true)/32768;
      async function run(priorities) {
        const worker=new Worker('./decoder-worker.js',{type:'module'}), channel=new MessageChannel();
        const events=[]; let resolveUpdate, rejectUpdate, resolveReady, resolvePriority;
        const ready=new Promise(r=>resolveReady=r), priority=new Promise(r=>resolvePriority=r);
        const done=new Promise((r,j)=>{resolveUpdate=r;rejectUpdate=j;});
        worker.onmessage=({data})=>{
          events.push({ ...data, at:performance.now() });
          if(data.kind==='ready')resolveReady();
          if(data.kind==='priorities-applied')resolvePriority();
          if(data.kind==='update')resolveUpdate(data.update);
          if(data.kind==='error')rejectUpdate(new Error(data.error));
        };
        worker.onerror=e=>rejectUpdate(new Error(e.message));
        try {
          worker.postMessage({rate:12000,epoch:1700000010000,port:channel.port2},[channel.port2]);
          await ready; worker.postMessage({kind:'priorities',frequencies:priorities}); await priority;
          channel.port1.postMessage({kind:'samples',frame:0,samples:pcm});
          const update=await Promise.race([done,new Promise((_,j)=>setTimeout(()=>j(new Error('WASM decode timed out')),45000))]);
          const start=events.find(e=>e.kind==='decode-started');
          const cq=events.find(e=>e.kind==='decode-found'&&e.decode.message==='CQ F5RXL IN94');
          const end=events.find(e=>e.kind==='decode-completed');
          return { first:events.find(e=>e.kind==='decode-found')?.decode.message, cqMs:cq.at-start.at, totalMs:end.at-start.at,
            emitted:events.filter(e=>e.kind==='decode-found').map(e=>e.decode.payload).sort(), payloads:update.decodes.map(d=>d.payload).sort(), cqBeforeComplete:cq.at<end.at };
        } finally { worker.terminate();channel.port1.close(); }
      }
      return { baseline:await run([]), prioritized:await run([1196.875]),
        crowded:await run(Array.from({length:100},(_,i)=>100+i*39)) };
    },wav);
    assert.equal(measurements.prioritized.first,'CQ F5RXL IN94');
    assert.equal(measurements.prioritized.cqBeforeComplete,true);
    assert.equal(measurements.prioritized.payloads.length,19);
    assert.deepEqual(measurements.prioritized.payloads,measurements.baseline.payloads);
    assert.deepEqual(measurements.prioritized.emitted,measurements.prioritized.payloads);
    assert.deepEqual(measurements.crowded.payloads,measurements.baseline.payloads);
    assert.deepEqual(measurements.crowded.emitted,measurements.crowded.payloads);
    assert.deepEqual(errors,[]);
    console.log(JSON.stringify({baselineCQms:measurements.baseline.cqMs,baselineDecodeMs:measurements.baseline.totalMs,priorityCQms:measurements.prioritized.cqMs,fullDecodeMs:measurements.prioritized.totalMs,crowdedDecodeMs:measurements.crowded.totalMs,payloads:measurements.prioritized.payloads.length}));
    console.log('PASS: queued frequency emitted first during ongoing release-WASM decode; complete independent fixture payload set retained');
  } finally {await browser.close();}
})().catch(e=>{console.error(e);process.exitCode=1;});

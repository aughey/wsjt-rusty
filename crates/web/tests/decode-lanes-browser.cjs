// Actual WASM lanes: freeze recovery's thread and verify two slot attempts still run.
const { chromium } = require('playwright');
const fs = require('node:fs');
const path = require('node:path');
const assert = require('node:assert/strict');
(async()=>{
  const browser=await chromium.launch({headless:true,args:['--no-sandbox']});
  try {
    const page=await browser.newPage();
    const errors=[];page.on('pageerror',e=>errors.push(e.message));
    await page.goto(process.argv[2]||'http://127.0.0.1:8083');
    const wav=fs.readFileSync(path.resolve(__dirname,'../../../tests/fixtures/ft8-210703-133430.wav')).toString('base64');
    const result=await page.evaluate(async wav=>{
      const bytes=Uint8Array.from(atob(wav),c=>c.charCodeAt(0)),view=new DataView(bytes.buffer);
      let data;
      for(let p=12;p+8<bytes.length;){const size=view.getUint32(p+4,true);if(String.fromCharCode(...bytes.slice(p,p+4))==='data')data={offset:p+8,size};p+=8+size+(size&1);}
      const pcm=new Float32Array(data.size/2);for(let i=0;i<pcm.length;i++)pcm[i]=view.getInt16(data.offset+i*2,true)/32768;
      const workerURL=new URL('./decoder-worker.js',location.href).href;
      const wrapper=URL.createObjectURL(new Blob([`import ${JSON.stringify(workerURL)}; const handler=self.onmessage; self.onmessage=async event=>{await handler(event);if(event.data.role==='recovery'){self.postMessage({kind:'stall-started'});const until=performance.now()+8000;while(performance.now()<until){}self.postMessage({kind:'stall-ended'});}};`],{type:'text/javascript'}));
      const recovery=new Worker(wrapper,{type:'module'}),slot=new Worker(workerURL,{type:'module'});
      const transport=new MessageChannel(),background=new MessageChannel();
      const epoch=1700000010000;
      const events=[],credits=[],recovered=new Set(),combined=new Set(),messages=new Map();
      let stallEnded=false;
      const waiters=new Map();
      function wait(key){return new Promise((resolve,reject)=>{const timer=setTimeout(()=>reject(new Error('Timeout: '+key)),45000);waiters.set(key,()=>{clearTimeout(timer);resolve();});});}
      const slotReady=wait('slot-ready'),stall=wait('stall'),priority=wait('priority'),firstCQ=wait('first-cq'),secondCQ=wait('second-cq'),recoveryDone=wait('recovery-done'),creditDone=wait('credits'),slotFinished=wait('slot-finished'),recoveryFinished=wait('recovery-finished');
      const signal=key=>{const fn=waiters.get(key);if(fn){waiters.delete(key);fn();}};
      transport.port1.onmessage=({data})=>{credits.push(data);if(credits.length===2)signal('credits');};transport.port1.start();
      recovery.onmessage=({data})=>{
        events.push({...data,lane:'recovery',at:performance.now()});
        if(data.kind==='stall-started')signal('stall');
        if(data.kind==='stall-ended')stallEnded=true;
        if(data.kind==='decode-found'){recovered.add(data.decode.payload);combined.add(data.decode.payload+':'+Math.round((data.decode.start_utc_ms-epoch)/15000));messages.set(data.decode.payload,data.decode.message);}
        if(data.kind==='update'&&data.update.received_seconds>=30)signal('recovery-done');
        if(data.kind==='finished')signal('recovery-finished');
        if(data.kind==='error')throw new Error(data.error);
      };
      slot.onmessage=({data})=>{
        events.push({...data,lane:'slot',at:performance.now(),recoveryStillBlocked:!stallEnded});
        if(data.kind==='ready')signal('slot-ready');
        if(data.kind==='priorities-applied')signal('priority');
        if(data.kind==='decode-found')combined.add(data.decode.payload+':'+Math.round((data.decode.start_utc_ms-epoch)/15000));
        if(data.kind==='decode-found'&&data.decode.message==='CQ F5RXL IN94')signal(data.decode.start_utc_ms<epoch+10000?'first-cq':'second-cq');
        if(data.kind==='finished')signal('slot-finished');
        if(data.kind==='error')throw new Error(data.error);
      };
      async function baseline(samples, chunks) {
        const worker=new Worker(workerURL,{type:'module'}), channel=new MessageChannel(), keys=new Set(), payloads=new Set();
        let readyResolve, updateResolve, finishResolve;
        const ready=new Promise(r=>readyResolve=r), done=new Promise(r=>finishResolve=r);
        worker.onmessage=({data})=>{
          if(data.kind==='ready')readyResolve();
          if(data.kind==='decode-found'){keys.add(data.decode.payload+':'+Math.round((data.decode.start_utc_ms-epoch)/15000));payloads.add(data.decode.payload);}
          if(data.kind==='update')updateResolve();
          if(data.kind==='finished')finishResolve();
          if(data.kind==='error')throw new Error(data.error);
        };
        try {
          worker.postMessage({rate:12000,epoch,port:channel.port2},[channel.port2]);await ready;
          let offset=0;
          for(const length of chunks){const updated=new Promise(r=>updateResolve=r);channel.port1.postMessage({kind:'samples',frame:offset,samples:samples.slice(offset,offset+length)});offset+=length;await updated;}
          channel.port1.postMessage({kind:'finish'});await done;
          return {keys:[...keys],payloads:[...payloads]};
        } finally {worker.terminate();channel.port1.close();}
      }
      // The exact independent 15 s fixture remains the 19-payload reference.
      // Joining two copies creates a new boundary; compare that derived input
      // against the unchanged combined receiver, rather than assuming its count.
      const original = await baseline(pcm,[pcm.length]);
      const repeated = new Float32Array(pcm.length*2);repeated.set(pcm);repeated.set(pcm,pcm.length);
      const reference = await baseline(repeated,[162000,198000]);
      try {
        recovery.postMessage({role:'recovery',rate:12000,epoch,port:background.port1},[background.port1]);
        slot.postMessage({role:'slot',rate:12000,epoch,port:transport.port2,recoveryPort:background.port2},[transport.port2,background.port2]);
        await Promise.all([slotReady,stall]);slot.postMessage({kind:'priorities',frequencies:[1196.875]});await priority;
        transport.port1.postMessage({kind:'samples',frame:0,samples:pcm.slice(0,162000)});
        await firstCQ;
        const earlyCredits=credits.length;
        const tail=new Float32Array(198000);tail.set(pcm.slice(162000));tail.set(pcm,18000);
        transport.port1.postMessage({kind:'samples',frame:162000,samples:tail});
        await secondCQ;
        const cqs=events.filter(e=>e.lane==='slot'&&e.kind==='decode-found'&&e.decode.message==='CQ F5RXL IN94');
        await Promise.all([recoveryDone,creditDone]);
        transport.port1.postMessage({kind:'finish'});await Promise.all([slotFinished,recoveryFinished]);
        return {earlyCredits,slotCQWhileRecoveryBlocked:cqs.map(e=>e.recoveryStillBlocked),recoveryPayloads:recovered.size,credits:credits.length, originalPayloads:original.payloads, referenceKeys:reference.keys, actualKeys:[...combined], recovered:[...recovered], messages:[...messages.values()],
          attempts:events.filter(e=>e.kind==='decode-completed').map(e=>({lane:e.lane,attempt:e.attempt,elapsed_ms:e.elapsed_ms}))};
      } finally {slot.terminate();recovery.terminate();transport.port1.close();URL.revokeObjectURL(wrapper);}
    },wav);
    assert.deepEqual(result.slotCQWhileRecoveryBlocked,[true,true]);
    assert.equal(result.earlyCredits,0,'transport credit waits for both lanes');assert.equal(result.credits,2);
    assert.equal(result.originalPayloads.length,19,'independent original fixture baseline remains unchanged');
    for(const payload of result.originalPayloads) assert.ok(result.recovered.includes(payload),'full recovery must retain each original fixture payload');
    assert.deepEqual(result.actualKeys.sort(),result.referenceKeys.sort(),'split lanes must preserve the exact combined-receiver payload/period set on the same derived input');
    assert.ok(result.attempts.some(e=>e.lane==='slot'&&e.attempt==='slot'));
    assert.ok(result.attempts.some(e=>e.lane==='recovery'&&e.attempt==='recovery'));
    assert.deepEqual(errors,[]);
    console.log(JSON.stringify({slotCQWhileRecoveryBlocked:result.slotCQWhileRecoveryBlocked,recoveryPayloads:result.recoveryPayloads,referenceEvents:result.referenceKeys.length,splitEvents:result.actualKeys.length,credits:result.credits,attempts:result.attempts}));
    console.log('PASS: two real-WASM slot CQs delivered while recovery was blocked for 8 s; full recovery, credits and both tails preserved');
  } finally {await browser.close();}
})().catch(e=>{console.error(e);process.exitCode=1;});

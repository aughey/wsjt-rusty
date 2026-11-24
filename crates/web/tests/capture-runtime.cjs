const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const source = fs.readFileSync(path.join(__dirname,'../src/capture-runtime.js'),'utf8');
const apiPromise = import(`data:text/javascript;base64,${Buffer.from(source).toString('base64')}`);
const flush = () => new Promise(setImmediate);
function setup({ permission, autoReady = true } = {}) {
  const workers=[], contexts=[], nodes=[], tracks=[];
  global.document={baseURI:'http://localhost/'};
  global.AudioContext=class {
    constructor(){this.state='suspended';this.sampleRate=48000;this.currentTime=1;this.audioWorklet={addModule:async()=>{}};contexts.push(this);}
    async resume(){this.state='running';}
    async close(){this.state='closed';}
    createAnalyser(){return {fftSize:0};}
    createMediaStreamSource(){return {connect(){},disconnect(){}};}
  };
  global.AudioWorkletNode=class {
    constructor(){nodes.push(this);this.port={postMessage:data=>{
      if(data.kind==='connect')this.sink=data.port;
      if(data.kind==='stop'){this.port.onmessage({data:{kind:'stopped'}});}
    }};}
    connect(){}
    disconnect(){this.sink?.close();}
  };
  global.Worker=class {
    constructor(){workers.push(this);}
    postMessage(data){if(data.kind==='priorities'){this.priorities=data.frequencies;return;}this.init=data;if(autoReady)queueMicrotask(()=>this.emit({kind:'ready'}));}
    emit(data){this.onmessage?.({data});}
    terminate(){this.terminated=true;this.init?.port?.close();this.init?.recoveryPort?.close();}
  };
  const track={stopped:false,stop(){this.stopped=true;},addEventListener(){}};tracks.push(track);
  const stream={getTracks:()=>[track],getAudioTracks:()=>[track]};
  Object.defineProperty(global,'navigator',{configurable:true,value:{mediaDevices:{getUserMedia:permission || (async()=>stream)}}});
  return {workers,contexts,nodes,tracks,stream};
}
test('Stop waits for both tails, merges results, and releases both workers and input',async()=>{
  const api=await apiPromise, s=setup(), events=[];
  const capture=await api.openCapture(json=>events.push(JSON.parse(json)),api.createCancellation());
  assert.deepEqual(s.workers.map(w=>w.init.role),['recovery','slot']);
  api.captureSetPriorities(capture,new Float32Array([1200]));
  assert.deepEqual(s.workers.map(w=>w.priorities),[[1200],[1200]]);
  const stopping=api.stopCapture(capture);let done=false;stopping.then(()=>done=true);
  assert.equal(s.tracks[0].stopped,true);
  s.workers[1].emit({kind:'finished',update:{decodes:[{payload:'slot'}]}});await flush();
  assert.equal(done,false);assert.equal(s.workers[0].terminated,undefined);
  s.workers[0].emit({kind:'update',update:{decodes:[{payload:'extra'}],capture_end_ms:42}});
  assert.equal(events.at(-1).kind,'recovery-results');
  s.workers[0].emit({kind:'finished',update:{decodes:[{payload:'recovery'}]}});
  await stopping;assert.equal(done,true);
  assert.deepEqual(events.at(-1).update.decodes.map(d=>d.payload),['slot','recovery']);
  assert.ok(s.workers.every(w=>w.terminated));assert.equal(s.contexts[0].state,'closed');
});

test('completed audio state transitions do not stop a running receive clock',async()=>{
  const api=await apiPromise, s=setup(), events=[];
  const capture=await api.openCapture(json=>events.push(JSON.parse(json)),api.createCancellation());
  // Queued statechange notifications must consult the current state, not stop
  // a context that is already running again.
  s.contexts[0].onstatechange();
  assert.equal(s.tracks[0].stopped,false);
  assert.equal(events.length,0);
  api.abortCapture(capture);
});

test('a real audio-clock suspension still releases input and flushes both lanes',async()=>{
  const api=await apiPromise, s=setup(), events=[];
  const capture=await api.openCapture(json=>events.push(JSON.parse(json)),api.createCancellation());
  s.contexts[0].state='suspended';
  s.contexts[0].onstatechange();
  assert.match(events[0].error,/clock suspended/i);
  assert.equal(s.tracks[0].stopped,true);
  for(const worker of s.workers)worker.emit({kind:'finished',update:{decodes:[]}});
  await api.stopCapture(capture);
  assert.ok(s.workers.every(w=>w.terminated));
});
test('permission cancellation releases a late stream; startup cancellation terminates both workers',async()=>{
  const api=await apiPromise;let grant;
  const s=setup({permission:()=>new Promise(resolve=>grant=resolve)}), token=api.createCancellation();
  const pending=api.openCapture(()=>{},token);await flush();api.cancelCapture(token);grant(s.stream);
  await assert.rejects(pending,/cancelled/i);assert.equal(s.tracks[0].stopped,true);assert.equal(s.contexts[0].state,'closed');
  const t=setup({autoReady:false}), next=api.createCancellation();
  const opening=api.openCapture(()=>{},next);await flush();assert.equal(t.workers.length,2);
  api.cancelCapture(next);await assert.rejects(opening,/cancelled/i);
  assert.ok(t.workers.every(w=>w.terminated));assert.equal(t.tracks[0].stopped,true);
});

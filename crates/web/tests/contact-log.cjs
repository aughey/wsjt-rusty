// Requires fake-indexeddb on NODE_PATH; SQLite itself is the shipped WASM.
const test=require('node:test');
const assert=require('node:assert/strict');
const fs=require('node:fs');
const path=require('node:path');
const vm=require('node:vm');
const {IDBFactory, IDBObjectStore}=require('fake-indexeddb');
const initSqlJs=require('../vendor/sql-wasm.js');
const source=fs.readFileSync(path.join(__dirname,'../contact-log-worker.js'),'utf8');
function worker(indexedDB) {
    let next=0; const pending=new Map();
    const scope={indexedDB,URL,Uint8Array,console, importScripts(){},
        initSqlJs:()=>initSqlJs({wasmBinary:fs.readFileSync(path.join(__dirname,'../vendor/sql-wasm.wasm'))}),
        location:{href:'http://localhost/contact-log-worker.js'},
        postMessage(data){const resolve=pending.get(data.id);pending.delete(data.id);resolve(structuredClone(data));},
    };
    scope.self=scope;vm.createContext(scope);vm.runInContext(source,scope);
    return data=>new Promise(resolve=>{const id=++next;pending.set(id,resolve);scope.onmessage({data:{id,...data}});});
}
function record(id='first') {
    const ms=Date.UTC(2026,10,22,23,59,55);
    return {id,revision:1,station_call:'W9XYZ',station_grid:'EN37',target:'K1ABC',grid:'FN42',
        start_ms:ms,end_ms:ms+20000,outcome:'Completed',evidence:[
            {start_ms:ms,end_ms:ms+12640,direction:'RX',message:'W9XYZ K1ABC -09',payload:'001',snr_db:-9,frequency_hz:1200},
            {start_ms:ms+15000,end_ms:ms+20000,direction:'TX audio',message:'K1ABC W9XYZ R-11',payload:null,snr_db:null,frequency_hz:1200},
        ]};
}
// A length-aware ADIF reader checks fields even when their values contain '<'.
function adifFields(text) {
    const fields=[];let p=0;
    while(p<text.length) {
        const start=text.indexOf('<',p);if(start<0)break;
        const end=text.indexOf('>',start), tag=text.slice(start+1,end).split(':');
        if(tag.length===1) {fields.push([tag[0],null]);p=end+1;}
        else {const n=Number(tag[1]);assert.ok(Number.isInteger(n));fields.push([tag[0],text.slice(end+1,end+1+n)]);p=end+1+n;}
    }
    return fields;
}
test('SQLite persists dates and evidence across workers, serializes tabs, and rejects stale revisions',async()=>{
    const idb=new IDBFactory(),a=worker(idb),b=worker(idb);
    assert.equal((await a({action:'list'})).result.total,0);
    await Promise.all([a({action:'save',records:[record('a')]}),b({action:'save',records:[record('b')]})]);
    const c=worker(idb), loaded=await c({action:'list'});
    assert.equal(loaded.result.total,2);
    assert.deepEqual(loaded.result.records[0].evidence,record().evidence);
    const latest={...record('a'),revision:3,outcome:'Interrupted'};
    await b({action:'save',records:[latest]});
    await a({action:'save',records:[record('a')]});
    const after=(await c({action:'list'})).result;
    assert.equal(after.records.find(r=>r.id==='a').outcome,'Interrupted');
    const bad={...record('corrupt'),start_ms:NaN};
    assert.match((await a({action:'save',records:[record('must-rollback'),bad]})).error,/Invalid contact/);
    assert.equal((await c({action:'list'})).result.total,2,'failed SQL/IDB transaction keeps the original database');
});
test('ADIF preserves UTC midnight, reports, status and length-delimited evidence without inventing RF frequency',async()=>{
    const request=worker(new IDBFactory()),r=record();
    r.evidence.push({...r.evidence[0],message:'Evidence <with> å symbol'});
    await request({action:'save',records:[r,{...record('incomplete'),outcome:'Interrupted'}]});
    const exported=(await request({action:'export'})).result;
    assert.equal(exported.count,1);
    const fields=adifFields(exported.text),get=name=>fields.find(f=>f[0]===name)?.[1];
    assert.equal(get('QSO_DATE'),'20261122');assert.equal(get('QSO_DATE_OFF'),'20261123');
    assert.equal(get('TIME_ON'),'235955');assert.equal(get('TIME_OFF'),'000015');
    assert.equal(get('RST_SENT'),'-11');assert.equal(get('RST_RCVD'),'-09');
    assert.equal(get('QSO_COMPLETE'),'?');assert.equal(get('FREQ'),undefined);assert.equal(get('BAND'),undefined);
    assert.deepEqual(JSON.parse(get('APP_WSJT_RUSTY_EVIDENCE')),[...r.evidence].sort((a,b)=>a.start_ms-b.start_ms));
    assert.equal((await request({action:'export',include_incomplete:true})).result.count,2);
});
test('ADIF exports a real radio dial readback and its query timestamp',async()=>{
    const request=worker(new IDBFactory()),r={...record(),dial_hz:14074000,frequency_read_ms:Date.UTC(2026,10,22,23,59,54)};
    await request({action:'save',records:[r]});
    const text=(await request({action:'export'})).result.text;
    assert.match(text,/<FREQ:9>14.074000/);
    assert.match(text,/APP_WSJT_RUSTY_FREQ_READ_UTC/);
    assert.doesNotMatch(text,/<FREQ:[^>]+>1200/);
});
test('a browser quota failure does not acknowledge or replace the saved database',async()=>{
    const request=worker(new IDBFactory());
    await request({action:'save',records:[record('saved')]});
    const put=IDBObjectStore.prototype.put;
    try {
        IDBObjectStore.prototype.put=function(){throw new Error('Quota exceeded');};
        assert.match((await request({action:'save',records:[record('unsaved')]})).error,/Quota exceeded/);
    } finally {IDBObjectStore.prototype.put=put;}
    const loaded=(await request({action:'list'})).result;
    assert.equal(loaded.total,1);assert.equal(loaded.records[0].id,'saved');
});
test('great-circle paths split across the date line with finite coordinates',()=>{
    const source=fs.readFileSync(path.join(__dirname,'../src/map-runtime.js'),'utf8').replaceAll('export ','');
    const scope={};vm.createContext(scope);vm.runInContext(source,scope);
    const parts=scope.communicationPath({latitude:35,longitude:170},{latitude:40,longitude:-170});
    assert.equal(parts.length,2);
    assert.ok(parts.flat().every(p=>p.every(Number.isFinite)));
    for(const part of parts)assert.ok(part.every((p,i)=>!i||Math.abs(p[0]-part[i-1][0])<=180));
});

test('completed history comes from the full durable log with the latest completion date per station',async()=>{
    const idb=new IDBFactory(),request=worker(idb),day=86400000;
    const old={...record('old'),end_ms:record().end_ms-8*day};
    const recent={...record('recent'),end_ms:record().end_ms-day};
    const failed={...record('failed'),target:'W1FAIL',outcome:'Interrupted'};
    const many=Array.from({length:30},(_,i)=>({...record(`other-${i}`),target:`K${i}XYZ`}));
    await request({action:'save',records:[old,recent,failed,...many]});
    const result=(await worker(idb)({action:'list',offset:25})).result;
    assert.ok(result.records.length<25);
    assert.deepEqual(result.contacted.find(r=>r.target==='K1ABC'),{target:'K1ABC',end_ms:recent.end_ms});
    assert.equal(result.contacted.filter(r=>r.target==='K1ABC').length,1);
    assert.equal(result.contacted.some(r=>r.target==='W1FAIL'),false);
    assert.equal(result.contacted.length,31,'exclusion is independent of the visible log page');
});

test('recontact preferences survive reload, validate stored values, and report storage failures',()=>{
    const runtime=fs.readFileSync(path.join(__dirname,'../src/contact-log-runtime.js'),'utf8').replaceAll('export ','');
    const values=new Map(),storage={getItem:k=>values.get(k)??null,setItem:(k,v)=>values.set(k,v)};
    const load=()=>{const scope={localStorage:storage};vm.createContext(scope);vm.runInContext(runtime,scope);return scope;};
    let scope=load();
    assert.equal(JSON.parse(scope.loadContactExclusion()).days,null);
    for(const days of [7,1,36500,0,null]) {
        assert.equal(scope.saveContactExclusion(JSON.stringify(days)),'');
        scope=load(); assert.equal(JSON.parse(scope.loadContactExclusion()).days,days);
    }
    for(const invalid of [-1,1.5,36501,'7',{},true]) {
        storage.setItem('wsjt-rusty-contact-exclusion-days',JSON.stringify(invalid));
        assert.match(JSON.parse(scope.loadContactExclusion()).error,/could not be restored/);
        assert.match(scope.saveContactExclusion(JSON.stringify(invalid)),/could not be saved/);
    }
    storage.setItem=()=>{throw new Error('quota');};
    assert.match(scope.saveContactExclusion('7'),/quota/);
});


test('interruption cause survives persistence, reload and incomplete ADIF export',async()=>{
    const idb=new IDBFactory(), request=worker(idb);
    const cause='Receive stopped: Audio clock discontinuity. Capture stopped.';
    const r={...record('interrupted-with-cause'),outcome:cause};
    await request({action:'save',records:[r]});
    const reloaded=worker(idb);
    assert.equal((await reloaded({action:'list'})).result.records[0].outcome,cause);
    assert.equal((await reloaded({action:'export'})).result.count,0);
    const exported=(await reloaded({action:'export',include_incomplete:true})).result;
    const fields=adifFields(exported.text);
    assert.equal(fields.find(f=>f[0]==='APP_WSJT_RUSTY_STATUS')[1],cause);
    assert.equal(fields.find(f=>f[0]==='QSO_COMPLETE')[1],'N');
});

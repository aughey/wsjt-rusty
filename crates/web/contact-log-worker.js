// SQLite runs outside the audio/UI workers. IndexedDB commits the database image
// atomically. Each operation reads the current image inside its transaction, so
// two open tabs cannot overwrite one another with an old SQLite snapshot.
importScripts('vendor/sql-wasm.js');
const sqlReady = initSqlJs({locateFile: file => new URL(`vendor/${file}`, self.location.href).href});
let storageReady;
function storage() {
    if (!storageReady) storageReady = new Promise((resolve, reject) => {
        const request = indexedDB.open('wsjt-rusty-contact-log', 1);
        request.onupgradeneeded = () => request.result.createObjectStore('sqlite');
        request.onsuccess = () => { request.result.onversionchange = () => request.result.close(); resolve(request.result); };
        request.onerror = () => { storageReady = undefined; reject(request.error); };
        request.onblocked = () => { storageReady = undefined; reject(new Error('Contact database is blocked by another tab. Close other receiver tabs and retry.')); };
    });
    return storageReady;
}
function rows(db, sql, params = []) {
    const statement = db.prepare(sql);
    try { statement.bind(params); const values=[]; while(statement.step()) values.push(statement.getAsObject()); return values; }
    finally { statement.free(); }
}
function schema(db) {
    const version=rows(db,'PRAGMA user_version')[0].user_version;
    if(version>1) throw new Error('Contact database was created by a newer application. It has not been changed.');
    db.run(`CREATE TABLE IF NOT EXISTS contacts (id TEXT PRIMARY KEY, revision INTEGER NOT NULL,
        target TEXT NOT NULL, start_ms REAL NOT NULL, outcome TEXT NOT NULL, body TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS evidence (contact_id TEXT NOT NULL, sequence INTEGER NOT NULL,
        start_ms REAL NOT NULL, direction TEXT NOT NULL, body TEXT NOT NULL, PRIMARY KEY(contact_id,sequence));
        CREATE INDEX IF NOT EXISTS contact_dates ON contacts(start_ms DESC);
        PRAGMA user_version=1;`);
}
function records(db, offset, limit) {
    return rows(db,`SELECT id,body FROM contacts ORDER BY start_ms DESC,id LIMIT ? OFFSET ?`,[limit,offset]).map(r => ({
        ...JSON.parse(r.body), evidence: rows(db,'SELECT body FROM evidence WHERE contact_id=? ORDER BY start_ms,sequence',[r.id]).map(e=>JSON.parse(e.body)),
    }));
}
function summary(db, offset = 0) {
    return {records:records(db,offset,25), total:rows(db,'SELECT count(*) AS n FROM contacts')[0].n,
        contacted:rows(db,"SELECT DISTINCT target FROM contacts WHERE outcome='Completed'").map(r=>r.target)};
}
async function execute(request) {
    const [SQL, store] = await Promise.all([sqlReady,storage()]);
    return new Promise((resolve,reject) => {
        const tx=store.transaction('sqlite',request.action==='save'?'readwrite':'readonly');
        const table=tx.objectStore('sqlite');
        let result, db, failure;
        tx.oncomplete=()=>{ db?.close(); resolve(result); };
        tx.onabort=()=>{ db?.close(); reject(failure || tx.error || new Error('Contact database transaction was aborted')); };
        tx.onerror=()=>{}; // onabort reports the actual persistence failure.
        const get=table.get('database-v1');
        get.onsuccess=()=>{
            try {
                db=get.result ? new SQL.Database(new Uint8Array(get.result)) : new SQL.Database();
                schema(db);
                if(request.action==='save') {
                    db.run('BEGIN');
                    for(const record of request.records) {
                        if(!record.id || !Number.isFinite(record.start_ms) || !Number.isFinite(record.end_ms)
                            || !Number.isSafeInteger(record.revision) || !Array.isArray(record.evidence) || record.evidence.length>256) throw new Error('Invalid contact record');
                        const old=rows(db,'SELECT revision FROM contacts WHERE id=?',[record.id])[0];
                        if(old && old.revision>=record.revision) continue;
                        const {evidence,...body}=record;
                        db.run('INSERT OR REPLACE INTO contacts VALUES(?,?,?,?,?,?)',[record.id,record.revision,record.target,record.start_ms,record.outcome,JSON.stringify(body)]);
                        db.run('DELETE FROM evidence WHERE contact_id=?',[record.id]);
                        evidence.forEach((e,i)=>db.run('INSERT INTO evidence VALUES(?,?,?,?,?)',[record.id,i,e.start_ms,e.direction,JSON.stringify(e)]));
                    }
                    db.run('COMMIT');
                    table.put(db.export(),'database-v1');
                    result=summary(db,request.offset || 0);
                } else if(request.action==='list') result=summary(db,request.offset || 0);
                else if(request.action==='export') {
                    const all=records(db,0,-1).filter(r=>request.include_incomplete || r.outcome==='Completed');
                    result={text:exportAdif(all), count:all.length};
                } else throw new Error('Unsupported contact log action');
            } catch(error) { failure=error; tx.abort(); }
        };
    });
}
let serial=Promise.resolve();
self.onmessage=({data})=>{
    serial=serial.catch(()=>{}).then(async()=>{
        try { self.postMessage({id:data.id,result:await execute(data)}); }
        catch(error) { self.postMessage({id:data.id,error:`Local contact database: ${error.message || error}`}); }
    });
};

function ascii(text) { return String(text).replace(/[^\x20-\x7e\r\n]/g,c=>'\\u'+c.charCodeAt(0).toString(16).padStart(4,'0')); }
function field(name,value) { value=ascii(value); return `<${name}:${value.length}>${value}`; }
function dateParts(ms) {
    const iso=new Date(ms).toISOString();
    return [iso.slice(0,10).replaceAll('-',''),iso.slice(11,19).replaceAll(':','')];
}
function exportAdif(records) {
    let text=field('ADIF_VER','3.1.7')+field('PROGRAMID','WSJT_RUSTY')+'<EOH>\n';
    for(const record of records) {
        const [date,time]=dateParts(record.start_ms), [endDate,endTime]=dateParts(record.end_ms);
        text+=field('CALL',record.target)+field('STATION_CALLSIGN',record.station_call)+field('MODE','FT8')
            +field('QSO_DATE',date)+field('TIME_ON',time)+field('QSO_DATE_OFF',endDate)+field('TIME_OFF',endTime);
        if(record.grid) text+=field('GRIDSQUARE',record.grid);
        if(record.station_grid) text+=field('MY_GRIDSQUARE',record.station_grid);
        // FREQ is the actual queried RF dial, following the FT8 dial convention.
        // Audio frequency is preserved separately in evidence, never substituted.
        if(Number.isFinite(record.dial_hz) && record.dial_hz>0) {
            text+=field('FREQ',(record.dial_hz/1000000).toFixed(6));
            if(Number.isFinite(record.frequency_read_ms)) text+=field('APP_WSJT_RUSTY_FREQ_READ_UTC',new Date(record.frequency_read_ms).toISOString());
        }
        for(const [direction,name] of [['TX audio','RST_SENT'],['RX','RST_RCVD']]) {
            const event=[...record.evidence].reverse().find(e=>e.direction===direction && (()=>{
                const w=e.message.split(/\s+/);
                return w[0]===(direction==='RX'?record.station_call:record.target)
                    && w[1]===(direction==='RX'?record.target:record.station_call) && /^R?[+-]\d\d$/.test(w[2]);
            })());
            if(event) text+=field(name,event.message.split(/\s+/)[2].replace(/^R/,''));
        }
        text+=field('QSO_COMPLETE',record.outcome==='Completed'?'?':'N')
            +field('COMMENT',`${record.outcome}; local software exchange state; RF delivery and mutual QSO completion not confirmed`)
            +field('APP_WSJT_RUSTY_ID',record.id)+field('APP_WSJT_RUSTY_STATUS',record.outcome)
            +field('APP_WSJT_RUSTY_EVIDENCE',JSON.stringify(record.evidence))+'<EOR>\n';
    }
    return text;
}

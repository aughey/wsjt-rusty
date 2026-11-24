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
// Load the same Rust log engine used by native SQLite, before opening the IDB transaction.
const engineReady = self.contactLogEngineReady || import('./wsjt-web.js').then(async module => {
    await module.default();
    return module;
});
async function execute(request) {
    const [SQL, store, rust] = await Promise.all([sqlReady,storage(),engineReady]);
    return new Promise((resolve,reject) => {
        const tx=store.transaction('sqlite',request.action==='save'?'readwrite':'readonly');
        const table=tx.objectStore('sqlite');
        let result, db, engine, failure;
        tx.oncomplete=()=>{ engine?.free(); db?.close(); resolve(result); };
        tx.onabort=()=>{ engine?.free(); db?.close(); reject(failure || tx.error || new Error('Contact database transaction was aborted')); };
        tx.onerror=()=>{}; // onabort reports the actual persistence failure.
        const get=table.get('database-v1');
        get.onsuccess=()=>{
            try {
                db=get.result ? new SQL.Database(new Uint8Array(get.result)) : new SQL.Database();
                engine = new rust.ContactLogEngine(db);
                result = JSON.parse(engine.request(JSON.stringify(request)));
                if (request.action === 'save') table.put(db.export(),'database-v1');
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

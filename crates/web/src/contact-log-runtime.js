let worker, next=0;
const pending=new Map();
function request(data) {
    if(!worker) {
        worker=new Worker(new URL('contact-log-worker.js',document.baseURI));
        worker.onmessage=({data})=>{
            const call=pending.get(data.id); if(!call) return;
            pending.delete(data.id); clearTimeout(call.timer);
            if(data.error) call.reject(new Error(data.error)); else call.resolve(data.result);
        };
        worker.onerror=event=>fail(new Error(`Contact database worker failed: ${event.message || 'could not load'}`));
    }
    return new Promise((resolve,reject)=>{
        const id=++next;
        const timer=setTimeout(()=>fail(new Error('Contact database timed out. Unsaved records are retained; retry saving.')),30000);
        pending.set(id,{resolve,reject,timer}); worker.postMessage({id,...data});
    });
}
function fail(error) {
    worker?.terminate(); worker=undefined;
    for(const call of pending.values()) {clearTimeout(call.timer);call.reject(error);} pending.clear();
}
export async function listContacts(offset) {return JSON.stringify(await request({action:'list',offset}));}
export async function saveContacts(json,offset) {return JSON.stringify(await request({action:'save',records:JSON.parse(json),offset}));}
export async function downloadAdif(includeIncomplete) {
    const result=await request({action:'export',include_incomplete:includeIncomplete});
    const url=URL.createObjectURL(new Blob([result.text],{type:'text/plain;charset=utf-8'}));
    const anchor=document.createElement('a'); anchor.href=url; anchor.download=`wsjt-rusty-contacts-${new Date().toISOString().slice(0,10)}.adi`;
    document.body.append(anchor); anchor.click(); anchor.remove(); setTimeout(()=>URL.revokeObjectURL(url),1000);
    return `${result.count} contact records exported as ADIF.`;
}
export function disposeContactLog() {fail(new Error('Contact log closed; unacknowledged records were not discarded.'));}

const exclusionKey = 'wsjt-rusty-contact-exclusion-days';
export function loadContactExclusion() {
    try {
        const raw = localStorage.getItem(exclusionKey);
        const days = raw === null ? null : JSON.parse(raw);
        if (days !== null && (!Number.isInteger(days) || days < 0 || days > 36500))
            throw new Error('Invalid saved recontact interval. Choose and save it again.');
        return JSON.stringify({days, error:''});
    } catch (error) {
        return JSON.stringify({days:null, error:`Recontact setting could not be restored: ${error.message}`});
    }
}
export function saveContactExclusion(json) {
    try {
        const days = JSON.parse(json);
        if (days !== null && (!Number.isInteger(days) || days < 0 || days > 36500)) throw new Error('Invalid interval');
        localStorage.setItem(exclusionKey, JSON.stringify(days));
        return '';
    } catch (error) { return `Recontact setting could not be saved: ${error.message}`; }
}

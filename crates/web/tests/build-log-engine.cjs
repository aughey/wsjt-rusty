// Compile the actual browser Rust engine for Node-based SQL/IndexedDB tests.
const fs=require('node:fs'),path=require('node:path'),os=require('node:os'),{spawnSync}=require('node:child_process');
const root=path.resolve(__dirname,'../../..');
const lock=fs.readFileSync(path.join(root,'Cargo.lock'),'utf8');
const version=lock.match(/name = "wasm-bindgen"\r?\nversion = "([^"]+)"/)[1];
const candidates=[process.env.WSJT_WASM_BINDGEN,'wasm-bindgen',path.join(process.env.LOCALAPPDATA||path.join(os.homedir(),'AppData','Local'),'trunkrs','trunk','cache',`wasm-bindgen-${version}`,'wasm-bindgen.exe')].filter(Boolean);
const bindgen=candidates.find(bin=>spawnSync(bin,['--version'],{encoding:'utf8'}).stdout?.trim()===`wasm-bindgen ${version}`);
if(!bindgen)throw new Error(`Use wasm-bindgen ${version}, or set WSJT_WASM_BINDGEN to that CLI executable.`);
function run(bin,args){const r=spawnSync(bin,args,{cwd:root,stdio:'inherit'});if(r.status!==0)process.exit(r.status||1);}
run('cargo',['build','-p','wsjt-web','--release','--target','wasm32-unknown-unknown']);
run(bindgen,['--target','nodejs','--out-dir','target/log-node','target/wasm32-unknown-unknown/release/wsjt_web.wasm']);

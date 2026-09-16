import {readFile, mkdtemp, rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {createWasiHost,assertIsolatedImports} from '../wasi/host.mjs';
import { WASI } from 'node:wasi';
import { auditImports, auditMemory, MEMORY_BYTES } from './wasm_contract.mjs';
if (process.argv.length !== 4 || !['baseline', 'wrapped', 'source', 'isolated'].includes(process.argv[3]))
  throw new Error('provide a managed core Wasm module and probe mode');
const pin = JSON.parse(await readFile(new URL('./toolchain.json', import.meta.url), 'utf8'));
if (process.versions.node !== pin.node_version) throw new Error('use audited Node ' + pin.node_version);
const mode = process.argv[3];
const workspace = await mkdtemp(join(tmpdir(),'dotnet-pal-wasi-'));
const wasi = new WASI({version:'preview1',args:['LlvmGcProbe',mode],
  env:{PAL_BCL_CANARY:'real-wasi-environment', PAL_WORKSPACE:'/workspace'},
  preopens:{'/workspace':workspace}, returnOnExit:true});
const bytes = await readFile(process.argv[2]);
const limits = auditMemory(bytes);
const module = await WebAssembly.compile(bytes);
const imports = WebAssembly.Module.imports(module);
console.log('IMPORTS', JSON.stringify(imports));
if (mode==='isolated') assertIsolatedImports(module); else auditImports(imports);
console.log('WASM MEMORY LIMIT PREFLIGHT PASS maximum_bytes=' + MEMORY_BYTES);
let instance;
const host = mode==='isolated' ? createWasiHost(wasi.wasiImport,()=>instance.exports.memory) : null;
instance = await WebAssembly.instantiate(module, host ? host.imports : {wasi_snapshot_preview1:wasi.wasiImport});
if (instance.exports.memory.buffer instanceof SharedArrayBuffer) throw new Error('this profile is single threaded');
let result;
try { result = wasi.start(instance); } finally { await rm(workspace,{recursive:true,force:true}); }
if (result !== 0) throw new Error('managed process failed ' + result);
const memory = instance.exports.memory;
if (memory.buffer.byteLength > MEMORY_BYTES) throw new Error('module exceeded its memory budget');
// A beyond-maximum growth must fail without allocating any additional pages.
let rejected = false;
try { memory.grow(limits.maxPages - memory.buffer.byteLength / 65536 + 1); }
catch (error) { if (!(error instanceof RangeError)) throw error; rejected = true; }
if (!rejected) throw new Error('engine did not enforce audited memory maximum');
if (host) {
  for(const name of ['fd_write','fd_read','path_open','path_rename','fd_readdir','random_get','clock_time_get','environ_get'])
    if (!(host.counts.get(name)>0)) throw new Error('missing actual BCL/GC host call: '+name);
  console.log('WASM OS ISOLATION PASS',JSON.stringify(Object.fromEntries(host.counts)));
}
console.log('WASM MODULE EXECUTION PASS mode=' + mode + ' memory_bytes=' + memory.buffer.byteLength);

import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {WASI} from 'node:wasi';
import {createWasiHost,assertIsolatedImports} from '../integration/wasi/host.mjs';
const module=await WebAssembly.compile(await readFile(process.argv[2]));
assertIsolatedImports(module);
await assert.rejects(WebAssembly.instantiate(module,{}));
for(let mode=0;mode<3;++mode) {
    const wasi=new WASI({version:'preview1',args:[],env:{},preopens:{}});
    let instance;
    const provider={...wasi.wasiImport};
    if(mode===1) provider.random_get=()=>65536;
    if(mode===2) provider.proc_exit=()=>0; // a returning exit is a broken host
    const host=createWasiHost(provider,()=>instance.exports.memory);
    instance=await WebAssembly.instantiate(module,host.imports);
    wasi.initialize(instance);
    if(mode===2) assert.throws(()=>instance.exports.pal_bridge_test(mode),WebAssembly.RuntimeError);
    else assert.equal(instance.exports.pal_bridge_test(mode),0,`mode ${mode}`);
}
console.log('WASI BRIDGE PASS one physical host import, real clock/entropy, malformed requests, non-returning exit enforced');

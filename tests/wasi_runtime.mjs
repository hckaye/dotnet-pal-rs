import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {WASI} from 'node:wasi';
const module = await WebAssembly.compile(await readFile(process.argv[2]));
const names = ['clock_time_get', 'environ_get', 'environ_sizes_get', 'random_get'];
const actual = WebAssembly.Module.imports(module);
assert.deepEqual(actual.map(x=>[x.module,x.name,x.kind]).sort(), names.map(x=>['wasi_snapshot_preview1',x,'function']).sort());
await assert.rejects(WebAssembly.instantiate(module, {}));
for (let mode=0;mode<=6;++mode) {
    const wasi = new WASI({version:'preview1', args:[], env:{PAL_RUNTIME_VALUE:'abc',PAL_RUNTIME_EMPTY:''}, preopens:{}});
    let instance;
    const counts = Object.fromEntries(names.map(n=>[n,0]));
    const table = Object.fromEntries(names.map(name=>[name,(...args)=>{
        ++counts[name];
        const view = new DataView(instance.exports.memory.buffer);
        if (mode===1 && name==='environ_sizes_get') { view.setUint32(args[0]>>>0,99,true); return 29; }
        if (mode===2 && name==='random_get') { new Uint8Array(instance.exports.memory.buffer,args[0]>>>0,args[1]>>>0).fill(42); return 29; }
        if (mode===3 && name==='clock_time_get') { view.setBigUint64(args[2]>>>0,42n,true); return 29; }
        if (mode===4 && name==='environ_sizes_get') { view.setUint32(args[0]>>>0,257,true); view.setUint32(args[1]>>>0,20000,true); return 0; }
        if ((mode===5 || mode===6) && name==='environ_sizes_get') { view.setUint32(args[0]>>>0,1,true); view.setUint32(args[1]>>>0,3,true); return 0; }
        if ((mode===5 || mode===6) && name==='environ_get') {
            view.setUint32(args[0]>>>0,mode===5?0:args[1]>>>0,true);
            new Uint8Array(instance.exports.memory.buffer,args[1]>>>0,3).fill(65); return 0;
        }
        return wasi.wasiImport[name](...args);
    }]));
    instance = await WebAssembly.instantiate(module,{wasi_snapshot_preview1:table});
    wasi.initialize(instance);
    assert.equal(instance.exports.pal_wasi_runtime_test(mode),0,`mode ${mode}`);
    if (mode===4 || mode===1) assert.equal(counts.environ_get,0);
    if (mode===0) assert.equal(counts.random_get,1,'zero-size and invalid output must not enter the host');
    assert(!(instance.exports.memory.buffer instanceof SharedArrayBuffer));
}
console.log('WASI RUNTIME PASS real environment/realtime/entropy; malformed snapshots; sanitized failures; exact import allowlist');

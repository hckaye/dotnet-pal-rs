import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { WASI } from 'node:wasi';
const module = await WebAssembly.compile(await readFile(process.argv[2]));
assert.deepEqual(WebAssembly.Module.imports(module), [
  { module: 'wasi_snapshot_preview1', name: 'clock_time_get', kind: 'function' }
]);
// There must be no silent clock fallback when the embedding omits WASI.
await assert.rejects(WebAssembly.instantiate(module, {}));
const wasi = new WASI({ version: 'preview1', args: [], env: {}, preopens: {} });
let calls = 0;
const instance = await WebAssembly.instantiate(module, {
  wasi_snapshot_preview1: {
    clock_time_get(id, precision, out) {
      assert.equal(id, 1);
      assert.equal(precision, 1n);
      ++calls;
      return wasi.wasiImport.clock_time_get(id, precision, out);
    }
  }
});
wasi.initialize(instance);
assert(!(instance.exports.memory.buffer instanceof SharedArrayBuffer));
assert.equal(instance.exports.pal_clock_test(0), 0, 'real WASI clock contract');
assert.equal(calls, 2, 'invalid outputs must never reach the host');
let failedInstance;
let failedCalls = 0;
failedInstance = await WebAssembly.instantiate(module, {
  wasi_snapshot_preview1: {
    clock_time_get(_id, _precision, out) {
      ++failedCalls;
      new DataView(failedInstance.exports.memory.buffer).setBigUint64(out >>> 0, 123n, true);
      return 29; // WASI IO error; scribbled output must not leak through the boundary
    }
  }
});
assert.equal(failedInstance.exports.pal_clock_test(1), 0, 'WASI error propagation');
assert.equal(failedCalls, 1);
console.log('WASI CLOCK PASS real host import, missing-import rejection, injected error, capability isolation');

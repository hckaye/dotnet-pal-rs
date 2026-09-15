import { readFile } from 'node:fs/promises';
import { WASI } from 'node:wasi';
if (process.argv.length !== 4 || !['baseline', 'wrapped'].includes(process.argv[3]))
  throw new Error('provide a managed core Wasm module and probe mode');
const mode = process.argv[3];
const wasi = new WASI({ version: 'preview1', args: ['LlvmGcProbe', mode],
  env: { DOTNET_GCHeapHardLimit: '4000000' }, returnOnExit: true });
const module = await WebAssembly.compile(await readFile(process.argv[2]));
const imports = WebAssembly.Module.imports(module);
console.log('IMPORTS', JSON.stringify(imports));
for (const entry of imports) {
  if (entry.module !== 'wasi_snapshot_preview1') throw new Error('unexpected host dependency ' + entry.module + '.' + entry.name);
}
const instance = await WebAssembly.instantiate(module, { wasi_snapshot_preview1: wasi.wasiImport });
if (instance.exports.memory.buffer instanceof SharedArrayBuffer) throw new Error('this profile is single threaded');
const result = wasi.start(instance);
if (result !== 0) throw new Error('managed process failed ' + result);
console.log('WASM MODULE EXECUTION PASS mode=' + mode + ' memory_bytes=' + instance.exports.memory.buffer.byteLength);

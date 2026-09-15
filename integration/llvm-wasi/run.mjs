import { readFile } from 'node:fs/promises';
import { WASI } from 'node:wasi';
if (process.argv.length !== 3) throw new Error('provide a managed core Wasm module');
const wasi = new WASI({ version: 'preview1', args: ['LlvmGcProbe'],
  env: { DOTNET_GCHeapHardLimit: '4000000' }, returnOnExit: true });
const module = await WebAssembly.compile(await readFile(process.argv[2]));
const imports = WebAssembly.Module.imports(module);
console.log('IMPORTS', JSON.stringify(imports));
for (const entry of imports) {
  if (entry.module !== 'wasi_snapshot_preview1') throw new Error('unexpected host dependency ' + entry.module + '.' + entry.name);
}
const instance = await WebAssembly.instantiate(module, { wasi_snapshot_preview1: wasi.wasiImport });
const result = wasi.start(instance);
if (result !== 0) throw new Error('managed process failed ' + result);

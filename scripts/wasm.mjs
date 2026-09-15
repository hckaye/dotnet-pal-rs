import fs from 'node:fs';
import assert from 'node:assert/strict';

const path = process.argv[2];
if (!path) throw new Error('Usage: node scripts/wasm.mjs module.wasm');
const module = await WebAssembly.compile(fs.readFileSync(path));
assert.deepEqual(WebAssembly.Module.imports(module), [], 'no OS/allocator imports expected');
for (let instanceIndex = 0; instanceIndex < 2; instanceIndex++) {
  const instance = await WebAssembly.instantiate(module, {});
  assert(instance.exports.memory instanceof WebAssembly.Memory);
  assert(!(instance.exports.memory.buffer instanceof SharedArrayBuffer), 'arena memory must not be shared');
  assert.equal(typeof instance.exports.wasm_arena_probe, 'function');
  for (let pass = 0; pass < 3; pass++) {
    assert.equal(instance.exports.wasm_arena_probe(), 0, `instance ${instanceIndex}, pass ${pass}`);
  }
}
console.log(`PASS: actual ABI 2 WASM arena, two instances x three passes, no imports: ${path}`);
console.log('Rust backend execution only; not NativeAOT WASM code generation.');

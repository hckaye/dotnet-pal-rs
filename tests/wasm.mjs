import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
const [path] = process.argv.slice(2);
assert.ok(path, 'usage: node tests/wasm.mjs file.wasm');
const bytes = await readFile(path);
assert.ok(WebAssembly.validate(bytes), 'invalid Wasm binary');
const module = await WebAssembly.compile(bytes);
assert.deepEqual(WebAssembly.Module.imports(module), [], 'unexpected host/runtime imports');
for (let instanceIndex = 0; instanceIndex < 2; ++instanceIndex) {
    const instance = await WebAssembly.instantiate(module, {});
    const { pal_test, memory } = instance.exports;
    assert.equal(typeof pal_test, 'function');
    assert.ok(memory instanceof WebAssembly.Memory);
    assert.ok(memory.buffer instanceof ArrayBuffer, 'shared memory is not tested/supported here');
    const size = memory.buffer.byteLength;
    for (let run = 0; run < 3; ++run) {
        assert.equal(pal_test(), 0, `C contract failed (return value is tests/linear.c line), run ${run}`);
        assert.equal(memory.buffer.byteLength, size, 'bounded allocator unexpectedly grew memory');
    }
}
console.log(`WASM PASS ${path}: C -> Rust; two instances, repeated reuse, no imports, no memory growth`);

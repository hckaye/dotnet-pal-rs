import test from 'node:test';
import assert from 'node:assert/strict';
import { assertBrowserImports, createBrowserHost, IMPORT_MODULE, IMPORT_NAMES, STATUS } from '../host/host.mjs';

function host(extra = {}) {
    const memory = new WebAssembly.Memory({ initial: 2 });
    const stderr = [];
    const h = createBrowserHost({ getMemory: () => memory, env: { A: 'xyz', EMPTY: '' }, stderr: (t) => stderr.push(t), ...extra });
    return { memory, stderr, table: h.imports[IMPORT_MODULE], counts: h.counts };
}
const encoder = new TextEncoder();
function put(memory, offset, text) { new Uint8Array(memory.buffer).set(encoder.encode(text), offset); }

test('import table has exactly the published names', () => {
    const { table } = host();
    assert.deepEqual(Object.keys(table).sort(), [...IMPORT_NAMES].sort());
    assert.throws(() => assertBrowserImports({}));
});

test('clocks write little-endian nanoseconds and never move backwards', () => {
    let ms = 5.5;
    const { memory, table, counts } = host({ now: () => ms, wallClock: () => 1_700_000_000_000 });
    const view = new DataView(memory.buffer);
    assert.equal(table.monotonic_ns(64), STATUS.OK);
    assert.equal(view.getBigUint64(64, true), 5_500_000n);
    ms = 1; // a host clock that jumps back is clamped, not exposed
    assert.equal(table.monotonic_ns(64), STATUS.OK);
    assert.equal(view.getBigUint64(64, true), 5_500_000n);
    assert.equal(table.realtime_ns(72), STATUS.OK);
    assert.equal(view.getBigUint64(72, true), 1_700_000_000_000_000_000n);
    assert.equal(counts.monotonic_ns, 2);
    for (const bad of [65, 65536 - 4, -8, 1.5]) assert.equal(table.monotonic_ns(bad), STATUS.INVALID_ARGUMENT);
    assert.equal(host({ now: () => NaN }).table.monotonic_ns(64), STATUS.OS_ERROR);
    assert.equal(host({ wallClock: () => -1 }).table.realtime_ns(64), STATUS.OS_ERROR);
});

test('entropy is chunked below the Web Crypto limit and zeroed on failure', () => {
    const calls = [];
    const { memory, table } = host({ fill: (view) => { calls.push(view.length); view.fill(9); } });
    assert.equal(table.random_bytes(0, 70000), STATUS.OK);
    assert.deepEqual(calls, [65536, 4464]);
    assert.ok(new Uint8Array(memory.buffer, 0, 70000).every((b) => b === 9));
    assert.equal(table.random_bytes(65536 * 2 - 10, 20), STATUS.INVALID_ARGUMENT);
    const failing = host({ fill: (view) => { view.fill(1); throw new Error('no entropy'); } });
    assert.equal(failing.table.random_bytes(0, 16), STATUS.OS_ERROR);
    assert.ok(new Uint8Array(failing.memory.buffer, 0, 16).every((b) => b === 0));
});

test('environment lookup reports required length, NUL termination and NOT_FOUND', () => {
    const { memory, table } = host();
    const view = new DataView(memory.buffer);
    put(memory, 0, 'A');
    assert.equal(table.environment_get(0, 1, 512, 0, 256), STATUS.BUFFER_TOO_SMALL);
    assert.equal(view.getUint32(256, true), 4);
    assert.equal(table.environment_get(0, 1, 512, 4, 256), STATUS.OK);
    assert.deepEqual([...new Uint8Array(memory.buffer, 512, 4)], [120, 121, 122, 0]);
    put(memory, 0, 'EMPTY');
    assert.equal(table.environment_get(0, 5, 512, 1, 256), STATUS.OK);
    assert.equal(view.getUint32(256, true), 1);
    put(memory, 0, 'NOPE');
    assert.equal(table.environment_get(0, 4, 512, 16, 256), STATUS.NOT_FOUND);
    assert.equal(table.environment_get(0, 0, 512, 16, 256), STATUS.INVALID_ARGUMENT);
    assert.equal(table.environment_get(0, 1, 512, 16, 257), STATUS.INVALID_ARGUMENT); // misaligned required
    assert.equal(table.environment_get(0, 1, 65536 * 2 - 2, 16, 256), STATUS.INVALID_ARGUMENT);
    assert.throws(() => createBrowserHost({ getMemory: () => memory, env: { X: 1 } }), TypeError);
});

test('diagnostic output is decoded to the sink and reports full progress', () => {
    const { memory, table, stderr } = host();
    const view = new DataView(memory.buffer);
    put(memory, 0, 'héllo\n');
    assert.equal(table.write_stderr(0, 7, 256), STATUS.OK);
    assert.equal(view.getUint32(256, true), 7);
    assert.deepEqual(stderr, ['héllo\n']);
    assert.equal(table.write_stderr(0, 7, 258), STATUS.INVALID_ARGUMENT);
    const failing = host({ stderr: () => { throw new Error('closed'); } });
    assert.equal(failing.table.write_stderr(0, 7, 256), STATUS.OS_ERROR);
});

test('shared or missing memory is rejected before any write', () => {
    const shared = new WebAssembly.Memory({ initial: 1, maximum: 1, shared: true });
    const h = createBrowserHost({ getMemory: () => shared }).imports[IMPORT_MODULE];
    assert.equal(h.monotonic_ns(0), STATUS.INVALID_ARGUMENT);
    assert.equal(h.random_bytes(0, 8), STATUS.INVALID_ARGUMENT);
    const none = createBrowserHost({ getMemory: () => undefined }).imports[IMPORT_MODULE];
    assert.equal(none.write_stderr(0, 1, 8), STATUS.INVALID_ARGUMENT);
    assert.throws(() => createBrowserHost({}), TypeError);
});

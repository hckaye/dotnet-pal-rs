/** Shared verification logic for the browser profile. Runs unchanged under Node
 * and inside a real browser page: only Web APIs are used, no node: modules.
 * Every check throws on failure; the caller decides how to report it.
 */
import { assertBrowserImports, createBrowserHost, IMPORT_MODULE, STATUS } from './host.mjs';

const MIB = 1024 * 1024;
function check(condition, message) { if (!condition) throw new Error(message); }

/** Instantiate the module with a (possibly wrapped) host and run one C test mode. */
async function runMode(module, mode, options) {
    let instance;
    const stderrLines = [];
    const host = createBrowserHost({
        getMemory: () => instance.exports.memory,
        env: { PAL_BROWSER_VALUE: 'abc', PAL_BROWSER_EMPTY: '' },
        stderr: (text) => stderrLines.push(text),
    });
    const table = { ...host.imports[IMPORT_MODULE] };
    if (options.wrap) options.wrap(table, () => instance.exports.memory);
    instance = await WebAssembly.instantiate(module, { [IMPORT_MODULE]: table });
    const memory = instance.exports.memory;
    check(memory instanceof WebAssembly.Memory, 'module must export its memory');
    check(!(typeof SharedArrayBuffer !== 'undefined' && memory.buffer instanceof SharedArrayBuffer), 'shared memory is not supported');
    const before = memory.buffer.byteLength;
    const line = instance.exports.pal_browser_test(mode);
    check(line === 0, `mode ${mode}: tests/browser.c failed at line ${line}`);
    return { counts: host.counts, stderrLines, before, after: memory.buffer.byteLength };
}

/**
 * @param {BufferSource} bytes  the compiled tests/browser.c + Rust module
 * @param {{heap?: boolean}} options  heap: the browser-heap (memory.grow) variant
 */
export async function runProbe(bytes, options = {}) {
    check(WebAssembly.validate(bytes), 'invalid Wasm binary');
    const module = await WebAssembly.compile(bytes);
    assertBrowserImports(module);
    // No silent fallback exists: a page that omits the host cannot instantiate the module.
    let rejected = false;
    try { await WebAssembly.instantiate(module, {}); } catch { rejected = true; }
    check(rejected, 'instantiation without the browser host must fail');

    const full = await runMode(module, 0, {});
    check(full.counts.monotonic_ns === 2, `clock host calls: ${full.counts.monotonic_ns}`);
    check(full.counts.realtime_ns === 1, `wall clock host calls: ${full.counts.realtime_ns}`);
    check(full.counts.random_bytes === 2, `entropy host calls: ${full.counts.random_bytes} (zero-size must not enter the host)`);
    check(full.counts.environment_get === 4, `environment host calls: ${full.counts.environment_get}`);
    check(full.counts.write_stderr === 1, `stderr host calls: ${full.counts.write_stderr}`);
    check(full.stderrLines.length === 1 && full.stderrLines[0] === 'browser diagnostics\n', `stderr sink received ${JSON.stringify(full.stderrLines)}`);
    if (options.heap) {
        check(full.before < 2 * MIB, `browser-heap initial memory should be small, was ${full.before}`);
        check(full.after - full.before >= 3 * MIB, `memory.grow expected, ${full.before} -> ${full.after}`);
        check(full.after <= 6 * MIB, `linker maximum must bound growth, reached ${full.after}`);
    } else {
        check(full.before >= 8 * MIB, `static arena must be part of the initial memory, was ${full.before}`);
        check(full.after === full.before, 'the bounded arena must not grow memory');
    }

    // Injected host failures. Scribbled outputs and unknown statuses never leak through.
    await runMode(module, 1, { wrap(table, getMemory) {
        table.monotonic_ns = (out) => { new DataView(getMemory().buffer).setBigUint64(out >>> 0, 123n, true); return STATUS.OS_ERROR; };
        table.realtime_ns = (out) => { new DataView(getMemory().buffer).setBigUint64(out >>> 0, 456n, true); return STATUS.OS_ERROR; };
    } });
    await runMode(module, 2, { wrap(table, getMemory) {
        table.random_bytes = (out, size) => { new Uint8Array(getMemory().buffer, out >>> 0, size).fill(7); return STATUS.OS_ERROR; };
    } });
    await runMode(module, 3, { wrap(table, getMemory) {
        const inner = table.environment_get;
        table.environment_get = (...args) => { inner(...args); return 77; };
        table.monotonic_ns = (out) => { new DataView(getMemory().buffer).setBigUint64(out >>> 0, 1n, true); return 4294967295; };
    } });
    await runMode(module, 4, { wrap(table, getMemory) {
        table.write_stderr = (_data, _size, written) => { new DataView(getMemory().buffer).setUint32(written >>> 0, 1, true); return STATUS.OK; };
    } });
    await runMode(module, 5, { wrap(table, getMemory) {
        table.environment_get = (_n, _l, _o, _c, required) => { new DataView(getMemory().buffer).setUint32(required >>> 0, 4, true); return STATUS.OK; };
    } });
    return {
        heap: Boolean(options.heap),
        initialMemory: full.before,
        finalMemory: full.after,
        hostCalls: full.counts,
    };
}

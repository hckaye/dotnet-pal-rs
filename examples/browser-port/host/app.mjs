/** Runs the managed BrowserApp module with both hosts: the boundary's
 * `dotnet_pal_browser_v1` imports (host.mjs) and the WASI Preview 1 subset
 * (wasi.mjs). Shared by the Node runner and the generated page; Web APIs only.
 */
import { createBrowserHost, IMPORT_MODULE } from './host.mjs';
import { createWasiPreview1 } from './wasi.mjs';

function check(condition, message) { if (!condition) throw new Error(message); }

/** Import audit: exactly the two host modules, functions only, no imported memory. */
export function auditAppImports(module) {
    const imports = WebAssembly.Module.imports(module);
    const modules = new Set(imports.map((i) => i.module));
    check(modules.size === 2 && modules.has(IMPORT_MODULE) && modules.has('wasi_snapshot_preview1'), `unexpected import modules ${[...modules]}`);
    check(imports.every((i) => i.kind === 'function'), 'only function imports are accepted');
    return imports;
}

/**
 * @param {BufferSource} bytes  the published BrowserApp.wasm
 * @returns {Promise<{code:number, lines:string[], palCalls:object, wasiCalls:object, initialMemory:number, finalMemory:number}>}
 */
export async function runApp(bytes) {
    check(WebAssembly.validate(bytes), 'invalid Wasm binary');
    const module = await WebAssembly.compile(bytes);
    auditAppImports(module);
    let instance;
    const lines = [];
    const env = { PAL_BROWSER: 'page' };
    const pal = createBrowserHost({ getMemory: () => instance.exports.memory, env, stderr: (text) => lines.push('[pal-stderr] ' + text) });
    const wasi = createWasiPreview1({
        getMemory: () => instance.exports.memory, args: ['BrowserApp', 'browser'], env,
        write: (fd, text) => lines.push((fd === 2 ? '[stderr] ' : '') + text),
    });
    instance = await WebAssembly.instantiate(module, { ...pal.imports, ...wasi.imports });
    check(instance.exports.memory instanceof WebAssembly.Memory, 'module must export its memory');
    check(!(typeof SharedArrayBuffer !== 'undefined' && instance.exports.memory.buffer instanceof SharedArrayBuffer), 'shared memory is not supported');
    const initialMemory = instance.exports.memory.buffer.byteLength;
    const code = wasi.start(instance);
    return {
        code, lines, palCalls: pal.counts, wasiCalls: Object.fromEntries(wasi.counts),
        initialMemory, finalMemory: instance.exports.memory.buffer.byteLength,
    };
}

/** Throws unless the managed workload reported success through the PAL. */
export function assertAppPassed(result) {
    check(result.code === 0, `managed process exited with ${result.code}: ${result.lines.join(' | ')}`);
    const pass = result.lines.find((l) => l.startsWith('MANAGED BROWSER PASS'));
    check(pass !== undefined, `no pass line in ${JSON.stringify(result.lines)}`);
    check(result.palCalls.monotonic_ns > 0, 'the runtime clock never reached the page through the PAL');
    check(result.wasiCalls.fd_write > 0 && result.wasiCalls.clock_time_get > 0, 'BCL console/clock did not reach the WASI host');
    check(result.wasiCalls.path_open === undefined, 'a browser page has no files to open');
    return pass;
}

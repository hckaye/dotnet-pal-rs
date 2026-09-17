/** Reference JavaScript host for the `browser` Rust profile.
 *
 * Works unchanged in browsers and in Node: it only uses WebAssembly, DataView,
 * TextEncoder/TextDecoder, performance.now(), Date.now() and crypto.getRandomValues().
 * Every function is a typed import in module `dotnet_pal_browser_v1` and returns a
 * dotnet_pal status code. Pointers are wasm32 offsets that this host bounds-checks
 * against the live instance memory; shared memory is rejected. This validation
 * protects the host page from a misbehaving module, it is not a sandbox for the
 * module's own memory.
 */
export const STATUS = Object.freeze({
    OK: 0, UNSUPPORTED: 1, INVALID_ARGUMENT: 2, OS_ERROR: 3, OUT_OF_MEMORY: 4,
    BUFFER_TOO_SMALL: 7, NOT_FOUND: 8,
});
export const IMPORT_MODULE = 'dotnet_pal_browser_v1';
export const IMPORT_NAMES = Object.freeze(['monotonic_ns', 'realtime_ns', 'random_bytes', 'environment_get', 'write_stderr']);
const RANDOM_CHUNK = 65536; // Web Crypto rejects larger getRandomValues requests.

/** Reject anything but an exact, non-shared, function-only import set. */
export function assertBrowserImports(module) {
    const imports = WebAssembly.Module.imports(module);
    const names = imports.map((i) => `${i.module}.${i.name}:${i.kind}`).sort();
    const expected = IMPORT_NAMES.map((n) => `${IMPORT_MODULE}.${n}:function`).sort();
    if (names.length !== expected.length || names.some((n, i) => n !== expected[i])) {
        throw new Error(`unexpected browser import set: ${JSON.stringify(names)}`);
    }
    return imports;
}

/**
 * @param {object} options
 * @param {() => WebAssembly.Memory} options.getMemory  the instance memory (after instantiation)
 * @param {Record<string,string>} [options.env]         environment snapshot exposed to the module
 * @param {(text: string) => void} [options.stderr]     diagnostic sink; defaults to console.error
 * @param {() => number} [options.now]                  monotonic milliseconds; defaults to performance.now
 * @param {() => number} [options.wallClock]            Unix milliseconds; defaults to Date.now
 * @param {(view: Uint8Array) => void} [options.fill]   entropy; defaults to crypto.getRandomValues
 */
export function createBrowserHost(options) {
    const { getMemory } = options;
    if (typeof getMemory !== 'function') throw new TypeError('getMemory is required');
    const env = options.env ?? {};
    const stderr = options.stderr ?? ((text) => console.error(text));
    const now = options.now ?? (() => globalThis.performance.now());
    const wallClock = options.wallClock ?? (() => Date.now());
    const fill = options.fill ?? ((view) => globalThis.crypto.getRandomValues(view));
    const encoder = new TextEncoder();
    const decoder = new TextDecoder('utf-8', { fatal: false });
    const entries = Object.entries(env).map(([name, value]) => {
        if (typeof value !== 'string') throw new TypeError(`environment value for ${name} must be a string`);
        return { name: encoder.encode(name), value: encoder.encode(value) };
    });
    const counts = Object.fromEntries(IMPORT_NAMES.map((n) => [n, 0]));
    let lastMonotonic = 0n;

    /** A readable/writable byte range inside the current instance memory, or null. */
    function range(pointer, length, alignment = 1) {
        const memory = getMemory();
        if (!(memory instanceof WebAssembly.Memory)) return null;
        const buffer = memory.buffer;
        if (typeof SharedArrayBuffer !== 'undefined' && buffer instanceof SharedArrayBuffer) return null;
        if (!Number.isInteger(pointer) || !Number.isInteger(length) || length < 0) return null;
        const start = pointer >>> 0;
        if (start % alignment !== 0 || start + length > buffer.byteLength) return null;
        return { buffer, start, length };
    }
    function bytesEqual(a, b) {
        if (a.length !== b.length) return false;
        for (let i = 0; i < a.length; ++i) if (a[i] !== b[i]) return false;
        return true;
    }
    const table = {
        monotonic_ns(out) {
            ++counts.monotonic_ns;
            const slot = range(out, 8, 8);
            if (!slot) return STATUS.INVALID_ARGUMENT;
            const milliseconds = now();
            if (!Number.isFinite(milliseconds) || milliseconds < 0) return STATUS.OS_ERROR;
            let ns = BigInt(Math.floor(milliseconds * 1e6));
            if (ns < lastMonotonic) ns = lastMonotonic; // never go backwards, even if the host clock does
            lastMonotonic = ns;
            new DataView(slot.buffer).setBigUint64(slot.start, ns, true);
            return STATUS.OK;
        },
        realtime_ns(out) {
            ++counts.realtime_ns;
            const slot = range(out, 8, 8);
            if (!slot) return STATUS.INVALID_ARGUMENT;
            const milliseconds = wallClock();
            if (!Number.isFinite(milliseconds) || milliseconds < 0) return STATUS.OS_ERROR;
            new DataView(slot.buffer).setBigUint64(slot.start, BigInt(Math.floor(milliseconds)) * 1000000n, true);
            return STATUS.OK;
        },
        random_bytes(out, size) {
            ++counts.random_bytes;
            const slot = range(out, size);
            if (!slot) return STATUS.INVALID_ARGUMENT;
            const view = new Uint8Array(slot.buffer, slot.start, slot.length);
            try {
                for (let offset = 0; offset < view.length; offset += RANDOM_CHUNK) {
                    fill(view.subarray(offset, Math.min(offset + RANDOM_CHUNK, view.length)));
                }
            } catch {
                view.fill(0); // never leave a partially filled buffer looking valid
                return STATUS.OS_ERROR;
            }
            return STATUS.OK;
        },
        environment_get(namePointer, nameLength, out, capacity, required) {
            ++counts.environment_get;
            const name = range(namePointer, nameLength);
            const requiredSlot = range(required, 4, 4);
            const output = range(out, capacity);
            if (!name || !requiredSlot || !output || nameLength === 0) return STATUS.INVALID_ARGUMENT;
            const wanted = new Uint8Array(name.buffer, name.start, name.length);
            const entry = entries.find((e) => bytesEqual(e.name, wanted));
            if (!entry) return STATUS.NOT_FOUND;
            const needed = entry.value.length + 1; // includes the terminating NUL
            new DataView(requiredSlot.buffer).setUint32(requiredSlot.start, needed, true);
            if (capacity < needed) return STATUS.BUFFER_TOO_SMALL;
            const target = new Uint8Array(output.buffer, output.start, needed);
            target.set(entry.value);
            target[entry.value.length] = 0;
            return STATUS.OK;
        },
        write_stderr(data, size, written) {
            ++counts.write_stderr;
            const writtenSlot = range(written, 4, 4);
            const bytes = range(data, size);
            if (!writtenSlot || !bytes) return STATUS.INVALID_ARGUMENT;
            try {
                stderr(decoder.decode(new Uint8Array(bytes.buffer, bytes.start, bytes.length)));
            } catch {
                return STATUS.OS_ERROR;
            }
            new DataView(writtenSlot.buffer).setUint32(writtenSlot.start, size, true);
            return STATUS.OK;
        },
    };
    return { imports: { [IMPORT_MODULE]: table }, counts };
}

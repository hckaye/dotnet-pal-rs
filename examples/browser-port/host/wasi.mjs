/** Browser-side WASI Preview 1 host for a NativeAOT LLVM `wasi-wasm` application.
 *
 * The experimental compiler links the managed runtime and BCL against wasi-libc,
 * so the module imports `wasi_snapshot_preview1` functions. A page has no WASI
 * runtime; this module provides the subset the .NET runtime and BCL use at
 * startup and for console/clock/entropy work, and answers every file-system,
 * socket and process request with the WASI errno that means "not available"
 * rather than pretending a file exists. Everything here runs unchanged in Node,
 * which is how the reference tests exercise it.
 *
 * Time-based waits (`poll_oneoff` with clock subscriptions, used by
 * Thread.Sleep) spin on `performance.now()` because a browser main thread cannot
 * block: the wait is real, not skipped. `proc_exit` throws `ProcessExit`, which
 * the caller of the module's `_start` catches to read the exit code.
 */
const ERRNO = Object.freeze({
    SUCCESS: 0, TOOBIG: 1, ACCES: 2, BADF: 8, EXIST: 20, INVAL: 28, IO: 29, ISDIR: 31, NOENT: 44, NOSYS: 52,
    NOTDIR: 54, NOTSUP: 58, PERM: 63, SPIPE: 70, NOTCAPABLE: 76,
});
export { ERRNO };
export class ProcessExit extends Error {
    constructor(code) { super(`process exited with code ${code}`); this.code = code; }
}
const FILETYPE_CHARACTER_DEVICE = 2;
const CLOCK_REALTIME = 0;
const CLOCK_MONOTONIC = 1;
const EVENTTYPE_CLOCK = 0;

/**
 * @param {object} options
 * @param {() => WebAssembly.Memory} options.getMemory
 * @param {string[]} [options.args]
 * @param {Record<string,string>} [options.env]
 * @param {(fd: number, text: string) => void} [options.write]   stdout/stderr sink, default console.log/error
 * @param {(view: Uint8Array) => void} [options.fill]           entropy, default crypto.getRandomValues
 */
export function createWasiPreview1(options) {
    const { getMemory } = options;
    if (typeof getMemory !== 'function') throw new TypeError('getMemory is required');
    const encoder = new TextEncoder();
    const decoder = new TextDecoder('utf-8', { fatal: false });
    const args = (options.args ?? ['app']).map((a) => encoder.encode(a + '\0'));
    const envs = Object.entries(options.env ?? {}).map(([k, v]) => encoder.encode(`${k}=${v}\0`));
    const write = options.write ?? ((fd, text) => (fd === 2 ? console.error(text) : console.log(text)));
    const fill = options.fill ?? ((view) => globalThis.crypto.getRandomValues(view));
    const counts = new Map();
    const pending = { 1: '', 2: '' }; // line buffers so console output is not split mid-line
    function count(name) { counts.set(name, (counts.get(name) ?? 0) + 1); }
    function view() {
        const memory = getMemory();
        if (!(memory instanceof WebAssembly.Memory)) throw new TypeError('instance memory is not available');
        return new DataView(memory.buffer);
    }
    function bytes(pointer, length) { return new Uint8Array(getMemory().buffer, pointer >>> 0, length >>> 0); }
    function inBounds(pointer, length) { return Number.isInteger(pointer) && Number.isInteger(length) && pointer >= 0 && pointer + length <= getMemory().buffer.byteLength; }
    function copyList(list, pointers, buffer) {
        const v = view();
        let offset = buffer >>> 0;
        list.forEach((item, index) => {
            v.setUint32((pointers >>> 0) + index * 4, offset, true);
            bytes(offset, item.length).set(item);
            offset += item.length;
        });
        return ERRNO.SUCCESS;
    }
    function sizes(list, countPointer, sizePointer) {
        const v = view();
        v.setUint32(countPointer >>> 0, list.length, true);
        v.setUint32(sizePointer >>> 0, list.reduce((n, a) => n + a.length, 0), true);
        return ERRNO.SUCCESS;
    }
    function flush(fd, text, final) {
        pending[fd] += text;
        const lines = pending[fd].split('\n');
        pending[fd] = final ? '' : lines.pop();
        for (const line of final ? lines.filter((l, i, a) => l !== '' || i < a.length - 1) : lines) write(fd, line);
    }
    function nowNs(clock) {
        if (clock === CLOCK_REALTIME) return BigInt(Date.now()) * 1000000n;
        if (clock === CLOCK_MONOTONIC) return BigInt(Math.floor(globalThis.performance.now() * 1e6));
        return null;
    }
    const wrap = (name, fn) => (...a) => { count(name); try { return fn(...a); } catch (e) { if (e instanceof ProcessExit) throw e; return ERRNO.IO; } };
    const imports = {
        args_get: wrap('args_get', (pointers, buffer) => copyList(args, pointers, buffer)),
        args_sizes_get: wrap('args_sizes_get', (c, s) => sizes(args, c, s)),
        environ_get: wrap('environ_get', (pointers, buffer) => copyList(envs, pointers, buffer)),
        environ_sizes_get: wrap('environ_sizes_get', (c, s) => sizes(envs, c, s)),
        clock_res_get: wrap('clock_res_get', (clock, out) => {
            if (nowNs(clock) === null) return ERRNO.INVAL;
            view().setBigUint64(out >>> 0, clock === CLOCK_REALTIME ? 1000000n : 1000n, true);
            return ERRNO.SUCCESS;
        }),
        clock_time_get: wrap('clock_time_get', (clock, _precision, out) => {
            const ns = nowNs(clock);
            if (ns === null) return ERRNO.INVAL;
            view().setBigUint64(out >>> 0, ns, true);
            return ERRNO.SUCCESS;
        }),
        fd_write: wrap('fd_write', (fd, iovs, iovsLength, written) => {
            if (fd !== 1 && fd !== 2) return ERRNO.BADF;
            const v = view();
            let total = 0;
            let text = '';
            for (let i = 0; i < iovsLength; ++i) {
                const pointer = v.getUint32((iovs >>> 0) + i * 8, true);
                const length = v.getUint32((iovs >>> 0) + i * 8 + 4, true);
                if (!inBounds(pointer, length)) return ERRNO.INVAL;
                text += decoder.decode(bytes(pointer, length), { stream: true });
                total += length;
            }
            flush(fd, text, false);
            v.setUint32(written >>> 0, total, true);
            return ERRNO.SUCCESS;
        }),
        fd_read: wrap('fd_read', (fd, _iovs, _n, read) => {
            if (fd === 0) { view().setUint32(read >>> 0, 0, true); return ERRNO.SUCCESS; } // EOF on stdin
            return ERRNO.BADF;
        }),
        fd_close: wrap('fd_close', (fd) => (fd <= 2 ? ERRNO.SUCCESS : ERRNO.BADF)),
        fd_fdstat_get: wrap('fd_fdstat_get', (fd, out) => {
            if (fd > 2) return ERRNO.BADF;
            const v = view();
            v.setUint8(out >>> 0, FILETYPE_CHARACTER_DEVICE);
            v.setUint16((out >>> 0) + 2, 0, true);
            v.setBigUint64((out >>> 0) + 8, fd === 0 ? 2n : 64n, true); // rights: fd_read or fd_write
            v.setBigUint64((out >>> 0) + 16, 0n, true);
            return ERRNO.SUCCESS;
        }),
        fd_fdstat_set_flags: wrap('fd_fdstat_set_flags', (fd) => (fd <= 2 ? ERRNO.SUCCESS : ERRNO.BADF)),
        fd_prestat_get: wrap('fd_prestat_get', () => ERRNO.BADF), // no preopened directories
        fd_prestat_dir_name: wrap('fd_prestat_dir_name', () => ERRNO.BADF),
        fd_seek: wrap('fd_seek', (fd) => (fd <= 2 ? ERRNO.SPIPE : ERRNO.BADF)),
        fd_tell: wrap('fd_tell', (fd) => (fd <= 2 ? ERRNO.SPIPE : ERRNO.BADF)),
        fd_filestat_get: wrap('fd_filestat_get', (fd, out) => {
            if (fd > 2) return ERRNO.BADF;
            bytes(out, 64).fill(0);
            view().setUint8((out >>> 0) + 16, FILETYPE_CHARACTER_DEVICE);
            return ERRNO.SUCCESS;
        }),
        fd_sync: wrap('fd_sync', (fd) => (fd <= 2 ? ERRNO.SUCCESS : ERRNO.BADF)),
        fd_datasync: wrap('fd_datasync', (fd) => (fd <= 2 ? ERRNO.SUCCESS : ERRNO.BADF)),
        fd_advise: wrap('fd_advise', () => ERRNO.BADF),
        fd_allocate: wrap('fd_allocate', () => ERRNO.BADF),
        fd_pread: wrap('fd_pread', () => ERRNO.BADF),
        fd_pwrite: wrap('fd_pwrite', () => ERRNO.BADF),
        fd_readdir: wrap('fd_readdir', () => ERRNO.BADF),
        fd_renumber: wrap('fd_renumber', () => ERRNO.BADF),
        fd_filestat_set_size: wrap('fd_filestat_set_size', () => ERRNO.BADF),
        fd_filestat_set_times: wrap('fd_filestat_set_times', () => ERRNO.BADF),
        fd_fdstat_set_rights: wrap('fd_fdstat_set_rights', () => ERRNO.BADF),
        path_open: wrap('path_open', () => ERRNO.NOTCAPABLE),
        path_filestat_get: wrap('path_filestat_get', () => ERRNO.NOTCAPABLE),
        path_filestat_set_times: wrap('path_filestat_set_times', () => ERRNO.NOTCAPABLE),
        path_create_directory: wrap('path_create_directory', () => ERRNO.NOTCAPABLE),
        path_remove_directory: wrap('path_remove_directory', () => ERRNO.NOTCAPABLE),
        path_unlink_file: wrap('path_unlink_file', () => ERRNO.NOTCAPABLE),
        path_rename: wrap('path_rename', () => ERRNO.NOTCAPABLE),
        path_link: wrap('path_link', () => ERRNO.NOTCAPABLE),
        path_symlink: wrap('path_symlink', () => ERRNO.NOTCAPABLE),
        path_readlink: wrap('path_readlink', () => ERRNO.NOTCAPABLE),
        poll_oneoff: wrap('poll_oneoff', (subscriptions, events, count, written) => {
            const v = view();
            let deadline = null;
            let userdata = 0n;
            for (let i = 0; i < count; ++i) {
                const base = (subscriptions >>> 0) + i * 48;
                const type = v.getUint8(base + 8);
                if (type !== EVENTTYPE_CLOCK) return ERRNO.NOTSUP; // no fds to wait on in a page
                const clock = v.getUint32(base + 16, true);
                const timeout = v.getBigUint64(base + 24, true);
                const flags = v.getUint16(base + 40, true);
                const absolute = (flags & 1) !== 0;
                const target = absolute ? timeout : (nowNs(clock) ?? 0n) + timeout;
                if (deadline === null || target < deadline) { deadline = target; userdata = v.getBigUint64(base, true); }
                if (nowNs(clock) === null) return ERRNO.INVAL;
            }
            if (deadline === null) return ERRNO.INVAL;
            // A page cannot block; wait by spinning until the earliest clock subscription is due.
            while (BigInt(Math.floor(globalThis.performance.now() * 1e6)) < deadline && nowNs(CLOCK_MONOTONIC) < deadline) { /* spin */ }
            v.setBigUint64(events >>> 0, userdata, true);
            v.setUint16((events >>> 0) + 8, ERRNO.SUCCESS, true);
            v.setUint8((events >>> 0) + 10, EVENTTYPE_CLOCK);
            v.setUint32(written >>> 0, 1, true);
            return ERRNO.SUCCESS;
        }),
        proc_exit: wrap('proc_exit', (code) => { flush(1, '', true); flush(2, '', true); throw new ProcessExit(code >>> 0); }),
        proc_raise: wrap('proc_raise', () => ERRNO.NOSYS),
        sched_yield: wrap('sched_yield', () => ERRNO.SUCCESS),
        random_get: wrap('random_get', (out, size) => {
            if (!inBounds(out >>> 0, size >>> 0)) return ERRNO.INVAL;
            const target = bytes(out, size);
            for (let offset = 0; offset < target.length; offset += 65536) fill(target.subarray(offset, Math.min(offset + 65536, target.length)));
            return ERRNO.SUCCESS;
        }),
        sock_accept: wrap('sock_accept', () => ERRNO.NOTSUP),
        sock_recv: wrap('sock_recv', () => ERRNO.NOTSUP),
        sock_send: wrap('sock_send', () => ERRNO.NOTSUP),
        sock_shutdown: wrap('sock_shutdown', () => ERRNO.NOTSUP),
    };
    /** Runs `_start` and returns the exit code (0 when `_start` returns normally). */
    function start(instance) {
        try { instance.exports._start(); flush(1, '', true); flush(2, '', true); return 0; }
        catch (error) { if (error instanceof ProcessExit) return error.code; flush(1, '', true); flush(2, '', true); throw error; }
    }
    return { imports: { wasi_snapshot_preview1: imports }, counts, start };
}

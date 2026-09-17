import test from 'node:test';
import assert from 'node:assert/strict';
import { createWasiPreview1, ERRNO, ProcessExit } from '../host/wasi.mjs';

function host(extra = {}) {
    const memory = new WebAssembly.Memory({ initial: 2 });
    const lines = [];
    const wasi = createWasiPreview1({ getMemory: () => memory, args: ['app', 'x'], env: { A: '1' }, write: (fd, text) => lines.push([fd, text]), ...extra });
    return { memory, lines, w: wasi.imports.wasi_snapshot_preview1, wasi };
}

test('arguments and environment are laid out as NUL-terminated lists', () => {
    const { memory, w } = host();
    const v = new DataView(memory.buffer);
    assert.equal(w.args_sizes_get(0, 4), ERRNO.SUCCESS);
    assert.equal(v.getUint32(0, true), 2);
    assert.equal(v.getUint32(4, true), 6);
    assert.equal(w.args_get(16, 64), ERRNO.SUCCESS);
    assert.equal(v.getUint32(16, true), 64);
    assert.equal(v.getUint32(20, true), 68);
    assert.deepEqual([...new Uint8Array(memory.buffer, 64, 6)], [...Buffer.from('app\0x\0')]);
    assert.equal(w.environ_sizes_get(0, 4), ERRNO.SUCCESS);
    assert.equal(v.getUint32(4, true), 4);
});

test('stdout and stderr are line-buffered into the sink; other fds are BADF', () => {
    const { memory, lines, w } = host();
    const v = new DataView(memory.buffer);
    new Uint8Array(memory.buffer).set(Buffer.from('hel'), 256);
    new Uint8Array(memory.buffer).set(Buffer.from('lo\nrest'), 300);
    v.setUint32(128, 256, true); v.setUint32(132, 3, true);
    v.setUint32(136, 300, true); v.setUint32(140, 7, true);
    assert.equal(w.fd_write(1, 128, 2, 512), ERRNO.SUCCESS);
    assert.equal(v.getUint32(512, true), 10);
    assert.deepEqual(lines, [[1, 'hello']]);
    assert.equal(w.fd_write(7, 128, 1, 512), ERRNO.BADF);
    assert.throws(() => w.proc_exit(3), (e) => e instanceof ProcessExit && e.code === 3);
    assert.deepEqual(lines, [[1, 'hello'], [1, 'rest']]);
});

test('clocks, entropy and yield work; files, sockets and preopens are absent', () => {
    const { memory, w } = host({ fill: (view) => view.fill(5) });
    const v = new DataView(memory.buffer);
    assert.equal(w.clock_time_get(0, 1n, 0), ERRNO.SUCCESS);
    assert.ok(v.getBigUint64(0, true) > 946684800000000000n);
    assert.equal(w.clock_time_get(1, 1n, 8), ERRNO.SUCCESS);
    assert.equal(w.clock_time_get(9, 1n, 8), ERRNO.INVAL);
    assert.equal(w.random_get(64, 70000), ERRNO.SUCCESS);
    assert.ok(new Uint8Array(memory.buffer, 64, 70000).every((b) => b === 5));
    assert.equal(w.random_get(65536 * 2 - 4, 8), ERRNO.INVAL);
    assert.equal(w.sched_yield(), ERRNO.SUCCESS);
    assert.equal(w.fd_prestat_get(3, 0), ERRNO.BADF);
    assert.equal(w.path_open(3, 0, 0, 0, 0, 0n, 0n, 0, 0), ERRNO.NOTCAPABLE);
    assert.equal(w.sock_recv(), ERRNO.NOTSUP);
    assert.equal(w.fd_fdstat_get(1, 0), ERRNO.SUCCESS);
    assert.equal(v.getUint8(0), 2);
    assert.equal(w.fd_read(0, 0, 0, 16), ERRNO.SUCCESS);
    assert.equal(v.getUint32(16, true), 0);
});

test('a clock subscription in poll_oneoff really waits', () => {
    const { memory, w } = host();
    const v = new DataView(memory.buffer);
    v.setBigUint64(0, 42n, true); // userdata
    v.setUint8(8, 0); // clock
    v.setUint32(16, 1, true); // monotonic
    v.setBigUint64(24, 5_000_000n, true); // 5 ms relative
    v.setUint16(40, 0, true);
    const before = performance.now();
    assert.equal(w.poll_oneoff(0, 64, 1, 128), ERRNO.SUCCESS);
    assert.ok(performance.now() - before >= 4.5, 'waited');
    assert.equal(v.getBigUint64(64, true), 42n);
    assert.equal(v.getUint32(128, true), 1);
    v.setUint8(8, 1); // fd_read subscription: unsupported in a page
    assert.equal(w.poll_oneoff(0, 64, 1, 128), ERRNO.NOTSUP);
});

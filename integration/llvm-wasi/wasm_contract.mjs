// Audit the binary declaration as well as the engine-visible import surface.
export const MEMORY_BYTES = 128 * 1024 * 1024;
const imports = new Set(`args_get args_sizes_get clock_time_get environ_get environ_sizes_get
fd_close fd_fdstat_get fd_prestat_get fd_prestat_dir_name fd_seek fd_write
path_filestat_get path_unlink_file poll_oneoff proc_exit sched_yield random_get`.split(/\s+/));
// Exact expanded import set after adding real BCL file operations. The isolated
// profile never uses this allowlist: it requires one dotnet_pal_host import only.
const bclImports = new Set([...imports, ...`fd_advise fd_filestat_get fd_filestat_set_size
fd_filestat_set_times fd_pread fd_pwrite fd_read fd_readdir fd_sync path_create_directory
path_link path_open path_remove_directory path_rename`.split(/\s+/)]);
export function auditImports(entries, profile='core') {
  if (!['core','bcl'].includes(profile)) throw new Error('unknown import profile');
  const allowed = profile==='bcl' ? bclImports : imports;
  const seen = new Set();
  for (const item of entries) {
    if (item.module !== 'wasi_snapshot_preview1' || item.kind !== 'function' ||
        !allowed.has(item.name) || seen.has(item.name))
      throw new Error('unapproved or duplicate import ' + item.module + '.' + item.name);
    seen.add(item.name);
  }
  for (const name of allowed) if (!seen.has(name)) throw new Error('missing audited import ' + name);
}
export function memoryLimits(bytes) {
  const header = [0, 97, 115, 109, 1, 0, 0, 0];
  if (bytes.length < 8 || header.some((v, i) => bytes[i] !== v)) throw new Error('not a core Wasm v1 module');
  let pos = 8, end = bytes.length, memory;
  function uint() {
    let value = 0;
    for (let shift = 0; shift < 35; shift += 7) {
      if (pos >= end) throw new Error('truncated LEB128');
      const b = bytes[pos++];
      if (shift === 28 && b > 15) throw new Error('LEB128 overflow');
      value += (b & 127) * 2 ** shift;
      if (!(b & 128)) return value;
    }
    throw new Error('invalid LEB128');
  }
  while (pos < bytes.length) {
    end = bytes.length;
    const id = bytes[pos++], length = uint(), next = pos + length;
    if (next > bytes.length) throw new Error('truncated section');
    if (id === 5) {
      end = next;
      if (memory || uint() !== 1 || uint() !== 1) throw new Error('expected one non-shared wasm32 memory with a maximum');
      const min = uint(), max = uint();
      if (min > max || max > 65536 || pos !== end) throw new Error('invalid memory limits');
      memory = { minPages: min, maxPages: max };
    }
    pos = next;
  }
  if (!memory) throw new Error('missing defined memory');
  return memory;
}
export function auditMemory(bytes) {
  const limits = memoryLimits(bytes);
  if (limits.maxPages * 65536 !== MEMORY_BYTES) throw new Error('wrong module memory maximum');
  return limits;
}

//! Exercises the negotiated C table of the std port through its function pointers,
//! the way a NativeAOT adapter would, on whatever desktop OS runs the test.
use dotnet_pal_rs::{kernel, runtime, services, support, CAP_LINEAR, CAP_VM, INVALID_ARGUMENT, OK, UNSUPPORTED};
use std::{ffi::c_void, ptr, sync::atomic::{AtomicUsize, Ordering}};

fn api() -> &'static dotnet_pal_rs::Api {
    let api = dotnet_pal_std::api();
    assert!(!api.is_null());
    unsafe { &*api }
}

#[test]
fn negotiation_advertises_desktop_capabilities() {
    let api = api();
    assert_eq!(api.header.abi_version, 2);
    assert_eq!(api.header.struct_size as usize, std::mem::size_of::<dotnet_pal_rs::Api>());
    let caps = api.header.capabilities;
    assert_eq!(caps & (CAP_VM | CAP_LINEAR), CAP_VM);
    assert!(api.linear.allocate.is_none());
    for bit in [services::CAP_CLOCK, services::CAP_SCHEDULER, kernel::CAP_EVENTS, kernel::CAP_MUTEX, kernel::CAP_THREADS, kernel::CAP_TLS,
        kernel::CAP_STACK, runtime::CAP_ENVIRONMENT, runtime::CAP_IDENTITY, runtime::CAP_REALTIME, runtime::CAP_ENTROPY,
        runtime::CAP_NATIVE_MEMORY, runtime::CAP_MODULES, support::CAP_HEAP, support::CAP_RWLOCK, support::CAP_NAME, support::CAP_DIAGNOSTICS] {
        assert_eq!(caps & bit, bit, "capability {bit} missing");
    }
    for bit in [dotnet_pal_rs::topology::CAP, dotnet_pal_rs::process::CAP, dotnet_pal_rs::streams::CAP, dotnet_pal_rs::files::CAP] { assert_eq!(caps & bit, bit, "capability {bit} missing"); }
    assert_eq!(caps & dotnet_pal_rs::image::CAP != 0, cfg!(target_os = "linux"));
    assert_eq!(caps & dotnet_pal_rs::context::CAP, 0);
    assert_eq!(caps & dotnet_pal_rs::wasi::CAP, 0);
    assert!(api.context.install.is_none() && api.wasi.invoke.is_none());
    // The barrier is a probe result, never a fake fence.
    assert_eq!(caps & kernel::CAP_BARRIER != 0, api.kernel.process_barrier.is_some());
}

#[test]
fn virtual_memory_reserve_commit_zero_recommit() {
    let vm = &api().vm;
    let page = unsafe { vm.page_size.unwrap()() };
    assert!(page.is_power_of_two());
    let mut p: *mut c_void = ptr::null_mut();
    assert_eq!(unsafe { vm.reserve.unwrap()(0, 0, 0, &mut p) }, INVALID_ARGUMENT);
    assert_eq!(unsafe { vm.reserve.unwrap()(page, 0, 7, &mut p) }, UNSUPPORTED);
    assert_eq!(unsafe { vm.reserve.unwrap()(2 * page - 1, 16 * page, 0, &mut p) }, OK);
    assert_eq!(p as usize % (16 * page), 0);
    assert_eq!(unsafe { vm.commit.unwrap()(p, 2 * page) }, OK);
    unsafe { ptr::write_bytes(p.cast::<u8>(), 0xa5, 2 * page) };
    assert_eq!(unsafe { vm.commit.unwrap()(p, page) }, OK);
    assert_eq!(unsafe { p.cast::<u8>().read() }, 0xa5, "recommit preserves contents");
    assert_eq!(unsafe { vm.decommit.unwrap()(p, page) }, OK);
    assert_eq!(unsafe { vm.commit.unwrap()(p, page) }, OK);
    assert!(unsafe { std::slice::from_raw_parts(p.cast::<u8>(), page) }.iter().all(|b| *b == 0), "recommit is zero-filled");
    assert_eq!(unsafe { p.cast::<u8>().add(page).read() }, 0xa5, "neighbor page intact");
    assert_eq!(unsafe { vm.reset.unwrap()(p, page) }, OK);
    assert_eq!(unsafe { vm.release.unwrap()(p, 2 * page - 1) }, OK);
}

#[test]
fn clocks_scheduler_and_runtime_services() {
    let api = api();
    let (mut a, mut b) = (0u64, 0u64);
    assert_eq!(unsafe { api.services.monotonic_ns.unwrap()(&mut a) }, OK);
    assert_eq!(unsafe { api.services.sleep_ns.unwrap()(2_000_000) }, OK);
    assert_eq!(unsafe { api.services.monotonic_ns.unwrap()(&mut b) }, OK);
    assert!(b >= a + 2_000_000);
    assert_eq!(unsafe { api.services.yield_thread.unwrap()() }, OK);
    assert_eq!(unsafe { api.services.monotonic_ns.unwrap()(ptr::null_mut()) }, INVALID_ARGUMENT);
    let mut now = 0u64;
    assert_eq!(unsafe { api.runtime.realtime_ns.unwrap()(&mut now) }, OK);
    assert!(now > 946_684_800_000_000_000);
    let (mut pid, mut tid) = (0u64, 0u64);
    assert_eq!(unsafe { api.runtime.process_id.unwrap()(&mut pid) }, OK);
    assert_eq!(pid, std::process::id() as u64);
    assert_eq!(unsafe { api.runtime.thread_id.unwrap()(&mut tid) }, OK);
    assert_ne!(tid, 0);
    let mut bytes = [0u8; 70000];
    assert_eq!(unsafe { api.runtime.random_bytes.unwrap()(bytes.as_mut_ptr(), bytes.len()) }, OK);
    assert!(bytes[65536..].iter().any(|b| *b != 0));
    std::env::set_var("PAL_STD_VALUE", "abc");
    let mut value = [42u8; 8];
    let mut needed = 0usize;
    let name = b"PAL_STD_VALUE";
    assert_eq!(unsafe { api.runtime.environment_get.unwrap()(name.as_ptr(), name.len(), ptr::null_mut(), 0, &mut needed) }, runtime::BUFFER_TOO_SMALL);
    assert_eq!(needed, 4);
    assert_eq!(unsafe { api.runtime.environment_get.unwrap()(name.as_ptr(), name.len(), value.as_mut_ptr(), value.len(), &mut needed) }, OK);
    assert_eq!(&value[..4], b"abc\0");
    let missing = b"PAL_STD_MISSING";
    assert_eq!(unsafe { api.runtime.environment_get.unwrap()(missing.as_ptr(), missing.len(), value.as_mut_ptr(), value.len(), &mut needed) }, runtime::NOT_FOUND);
    let mut mapping: *mut c_void = ptr::null_mut();
    assert_eq!(unsafe { api.runtime.mapping_allocate.unwrap()(4096, runtime::READ | runtime::WRITE, &mut mapping) }, OK);
    unsafe { mapping.cast::<u8>().write(7) };
    assert_eq!(unsafe { api.runtime.mapping_protect.unwrap()(mapping, 4096, runtime::READ) }, OK);
    assert_eq!(unsafe { api.runtime.mapping_release.unwrap()(mapping, 4096) }, OK);
    let mut module: *mut c_void = ptr::null_mut();
    assert_eq!(unsafe { api.runtime.module_open.unwrap()(ptr::null(), 0, &mut module) }, OK);
    let mut info = runtime::ModuleInfo { base: ptr::null_mut(), name: ptr::null(), name_length: 0 };
    assert_eq!(unsafe { api.runtime.module_info.unwrap()(api as *const _ as *mut c_void, &mut info) }, OK);
    assert!(!info.base.is_null() && info.name_length > 0);
    assert_eq!(unsafe { api.runtime.module_close.unwrap()(module) }, OK);
}

unsafe extern "C" fn entry(arg: *mut c_void) -> *mut c_void {
    let counter = unsafe { &*arg.cast::<AtomicUsize>() };
    counter.fetch_add(1, Ordering::SeqCst);
    ptr::null_mut()
}
unsafe extern "C" fn destructor(value: *mut c_void) {
    unsafe { &*value.cast::<AtomicUsize>() }.fetch_add(100, Ordering::SeqCst);
}

#[test]
fn kernel_events_mutexes_threads_tls_and_stack() {
    let k = &api().kernel;
    let mut event: *mut c_void = ptr::null_mut();
    assert_eq!(unsafe { k.event_create.unwrap()(0, 0, &mut event) }, OK);
    assert_eq!(unsafe { k.event_wait.unwrap()(event, 0) }, kernel::TIMEOUT);
    assert_eq!(unsafe { k.event_wait.unwrap()(event, 1_000_000) }, kernel::TIMEOUT);
    assert_eq!(unsafe { k.event_set.unwrap()(event) }, OK);
    assert_eq!(unsafe { k.event_wait.unwrap()(event, kernel::INFINITE) }, OK);
    assert_eq!(unsafe { k.event_wait.unwrap()(event, 0) }, kernel::TIMEOUT, "auto-reset consumed the signal");
    assert_eq!(unsafe { k.event_destroy.unwrap()(event) }, OK);

    let mut mutex: *mut c_void = ptr::null_mut();
    assert_eq!(unsafe { k.mutex_create.unwrap()(1, &mut mutex) }, OK);
    assert_eq!(unsafe { k.mutex_lock.unwrap()(mutex) }, OK);
    assert_eq!(unsafe { k.mutex_lock.unwrap()(mutex) }, OK, "recursive");
    assert_eq!(unsafe { k.mutex_destroy.unwrap()(mutex) }, kernel::BUSY);
    assert_eq!(unsafe { k.mutex_unlock.unwrap()(mutex) }, OK);
    assert_eq!(unsafe { k.mutex_unlock.unwrap()(mutex) }, OK);
    assert_eq!(unsafe { k.mutex_unlock.unwrap()(mutex) }, dotnet_pal_rs::OS_ERROR, "not owned");
    assert_eq!(unsafe { k.mutex_destroy.unwrap()(mutex) }, OK);

    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let mut threads = Vec::new();
    for _ in 0..4 {
        let mut thread: *mut c_void = ptr::null_mut();
        assert_eq!(unsafe { k.thread_create.unwrap()(Some(entry), &COUNTER as *const _ as *mut c_void, 256 * 1024, &mut thread) }, OK);
        threads.push(thread);
    }
    for thread in threads { assert_eq!(unsafe { k.thread_join.unwrap()(thread) }, OK); }
    assert_eq!(COUNTER.load(Ordering::SeqCst), 4);

    let mut key: *mut c_void = ptr::null_mut();
    assert_eq!(unsafe { k.tls_create.unwrap()(Some(destructor), &mut key) }, OK);
    let mut value: *mut c_void = ptr::null_mut();
    assert_eq!(unsafe { k.tls_get.unwrap()(key, &mut value) }, OK);
    assert!(value.is_null());
    static TLS_VALUE: AtomicUsize = AtomicUsize::new(0);
    assert_eq!(unsafe { k.tls_set.unwrap()(key, &TLS_VALUE as *const _ as *mut c_void) }, OK);
    assert_eq!(unsafe { k.tls_get.unwrap()(key, &mut value) }, OK);
    assert_eq!(value as usize, &TLS_VALUE as *const _ as usize);
    let key_address = key as usize;
    unsafe extern "C" fn set_and_exit(arg: *mut c_void) -> *mut c_void {
        let k = &api().kernel;
        assert_eq!(unsafe { k.tls_set.unwrap()(arg, &TLS_VALUE as *const _ as *mut c_void) }, OK);
        ptr::null_mut()
    }
    let mut worker: *mut c_void = ptr::null_mut();
    assert_eq!(unsafe { k.thread_create.unwrap()(Some(set_and_exit), key_address as *mut c_void, 0, &mut worker) }, OK);
    assert_eq!(unsafe { k.thread_join.unwrap()(worker) }, OK);
    assert_eq!(TLS_VALUE.load(Ordering::SeqCst), 100, "destructor ran at thread exit");
    assert_eq!(unsafe { k.tls_destroy.unwrap()(key) }, OK);

    let (mut low, mut high): (*mut c_void, *mut c_void) = (ptr::null_mut(), ptr::null_mut());
    assert_eq!(unsafe { k.stack_bounds.unwrap()(&mut low, &mut high) }, OK);
    let here = &low as *const _ as usize;
    assert!((low as usize) < here && here < (high as usize), "current frame lies inside the reported stack");
    if let Some(barrier) = k.process_barrier { assert_eq!(unsafe { barrier() }, OK); }
}

#[test]
fn support_heap_rwlock_name_and_diagnostics() {
    let s = &api().support;
    let mut p: *mut c_void = ptr::null_mut();
    assert_eq!(unsafe { s.allocate.unwrap()(37, 1, &mut p) }, OK);
    assert!(unsafe { std::slice::from_raw_parts(p.cast::<u8>(), 37) }.iter().all(|b| *b == 0));
    unsafe { ptr::write_bytes(p.cast::<u8>(), 0x5a, 37) };
    let mut q: *mut c_void = ptr::null_mut();
    assert_eq!(unsafe { s.resize.unwrap()(p, 117, &mut q) }, OK);
    assert!(unsafe { std::slice::from_raw_parts(q.cast::<u8>(), 37) }.iter().all(|b| *b == 0x5a));
    assert_eq!(unsafe { s.release.unwrap()(q) }, OK);
    assert_eq!(unsafe { s.release.unwrap()(ptr::null_mut()) }, OK);
    let mut lock: *mut c_void = ptr::null_mut();
    assert_eq!(unsafe { s.rw_create.unwrap()(&mut lock) }, OK);
    assert_eq!(unsafe { s.rw_read.unwrap()(lock) }, OK);
    assert_eq!(unsafe { s.rw_read.unwrap()(lock) }, OK, "shared readers");
    assert_eq!(unsafe { s.rw_destroy.unwrap()(lock) }, kernel::BUSY);
    assert_eq!(unsafe { s.rw_unlock.unwrap()(lock) }, OK);
    assert_eq!(unsafe { s.rw_unlock.unwrap()(lock) }, OK);
    assert_eq!(unsafe { s.rw_write.unwrap()(lock) }, OK);
    assert_eq!(unsafe { s.rw_unlock.unwrap()(lock) }, OK);
    assert_eq!(unsafe { s.rw_destroy.unwrap()(lock) }, OK);
    let name = b"pal-std";
    assert_eq!(unsafe { s.thread_name.unwrap()(name.as_ptr(), name.len()) }, OK);
    let mut written = 0usize;
    let text = b"dotnet-pal-std diagnostics\n";
    assert_eq!(unsafe { s.write_stderr.unwrap()(text.as_ptr(), text.len(), &mut written) }, OK);
    assert_eq!(written, text.len());
}

#[test]
fn topology_and_process_answer_within_their_contracts() {
    let api = api();
    let t = &api.topology;
    let (mut max, mut count, mut current) = (0u32, 0u32, 0u32);
    assert_eq!(unsafe { t.cpu_max.unwrap()(&mut max) }, OK);
    assert_eq!(unsafe { t.cpu_count.unwrap()(&mut count) }, OK);
    assert!(max >= 1 && count >= 1 && count <= max, "cpu_max={max} cpu_count={count}");
    let status = unsafe { t.current_cpu.unwrap()(&mut current) };
    assert!(status == OK && current < max || status == UNSUPPORTED);
    let mut mask = [0u8; 64];
    let mut needed = 0usize;
    assert_eq!(unsafe { t.process_affinity.unwrap()(mask.as_mut_ptr(), mask.len(), &mut needed) }, OK);
    assert_eq!(needed, (max as usize).div_ceil(8));
    assert!(mask[..needed].iter().any(|b| *b != 0), "an affinity set that runs nowhere");
    let mut small = [0u8; 0];
    assert_eq!(unsafe { t.process_affinity.unwrap()(small.as_mut_ptr(), 0, &mut needed) }, dotnet_pal_rs::runtime::BUFFER_TOO_SMALL);
    assert_eq!(unsafe { t.process_affinity.unwrap()(mask.as_mut_ptr(), mask.len(), ptr::null_mut()) }, INVALID_ARGUMENT);
    let (mut total, mut available) = (0u64, 0u64);
    assert_eq!(unsafe { t.physical_memory.unwrap()(&mut total, &mut available) }, OK);
    assert!(total > 0 && available <= total, "total={total} available={available}");
    let (mut limit, mut virtual_limit, mut cache) = (0u64, 0u64, 0usize);
    assert_eq!(unsafe { t.memory_limit.unwrap()(&mut limit) }, OK);
    assert!(limit == 0 || limit <= total);
    assert_eq!(unsafe { t.virtual_limit.unwrap()(&mut virtual_limit) }, OK);
    assert_eq!(unsafe { t.cache_size.unwrap()(&mut cache) }, OK);
    let (mut first, mut second) = (0u64, 0u64);
    assert_eq!(unsafe { t.cpu_features.unwrap()(&mut first, &mut second) }, OK);
    assert_eq!(unsafe { t.cpu_features.unwrap()(ptr::null_mut(), &mut second) }, INVALID_ARGUMENT);
    let p = &api.process;
    let mut present = 7u32;
    let status = unsafe { p.debugger_present.unwrap()(&mut present) };
    assert!(status == OK && present <= 1 || status == UNSUPPORTED && present == 0);
    let argv = [c"createdump".as_ptr().cast::<u8>()];
    let mut error = [0u8; 64];
    assert_eq!(unsafe { p.crash_dump.unwrap()(argv.as_ptr(), 1, error.as_mut_ptr(), error.len()) }, UNSUPPORTED);
    assert_eq!(unsafe { p.crash_dump.unwrap()(ptr::null(), 0, error.as_mut_ptr(), error.len()) }, INVALID_ARGUMENT);
    assert!(p.exit.is_some());
}

#[cfg(target_os = "linux")]
#[test]
fn image_lookups_describe_this_executable() {
    let api = api();
    let i = &api.image;
    let address = image_lookups_describe_this_executable as usize;
    let mut info = dotnet_pal_rs::image::UnwindInfo::default();
    assert_eq!(unsafe { i.unwind_info.unwrap()(address, &mut info, std::mem::size_of_val(&info)) }, OK);
    assert!(info.text_start <= address && address < info.text_start + info.text_length);
    assert!(info.eh_frame_hdr != 0 && info.eh_frame_hdr_length > 0);
    assert_eq!(unsafe { i.unwind_info.unwrap()(0, &mut info, std::mem::size_of_val(&info)) }, INVALID_ARGUMENT);
    assert_eq!(unsafe { i.readable.unwrap()(address, 16) }, OK);
    let page = unsafe { api.vm.page_size.unwrap()() };
    let mut reserved: *mut c_void = ptr::null_mut();
    assert_eq!(unsafe { api.vm.reserve.unwrap()(page, page, 0, &mut reserved) }, OK);
    assert_eq!(unsafe { i.readable.unwrap()(reserved as usize, page) }, dotnet_pal_rs::runtime::NOT_FOUND);
    assert_eq!(unsafe { api.vm.release.unwrap()(reserved, page) }, OK);
    let mut id = [0u8; 64];
    let mut needed = 0usize;
    let status = unsafe { i.build_id.unwrap()(info.base.max(1), id.as_mut_ptr(), id.len(), &mut needed) };
    assert!(status == OK && needed > 0 || status == dotnet_pal_rs::runtime::NOT_FOUND, "status {status}");
}

#[test]
fn streams_write_and_query_the_console() {
    let s = &api().streams;
    let mut written = 0usize;
    assert_eq!(unsafe { s.write.unwrap()(2, b"streams test line\n".as_ptr(), 18, &mut written) }, OK);
    assert_eq!(written, 18);
    assert_eq!(unsafe { s.write.unwrap()(0, b"x".as_ptr(), 1, &mut written) }, INVALID_ARGUMENT);
    assert_eq!(unsafe { s.write.unwrap()(1, ptr::null(), 1, &mut written) }, INVALID_ARGUMENT);
    assert_eq!(unsafe { s.write.unwrap()(1, b"x".as_ptr(), 0, &mut written) }, OK);
    let mut terminal = 7u32;
    assert_eq!(unsafe { s.is_terminal.unwrap()(1, &mut terminal) }, OK);
    assert!(terminal <= 1);
    assert_eq!(unsafe { s.is_terminal.unwrap()(3, &mut terminal) }, INVALID_ARGUMENT);
    let mut got = 9usize;
    let mut byte = 0u8;
    assert_eq!(unsafe { s.read.unwrap()(1, &mut byte, 1, &mut got) }, INVALID_ARGUMENT);
    assert_eq!(unsafe { s.read.unwrap()(0, &mut byte, 0, &mut got) }, OK);
    assert_eq!(got, 0);
}

mod files_table {
    use super::api;
    use dotnet_pal_rs::files::{self, Status, CREATE, EXCLUSIVE, LOCK_EXCLUSIVE, LOCK_SHARED, LOCK_UNLOCK, NODE_DIRECTORY, NODE_FILE, READ, TRUNCATE, WRITE};
    use dotnet_pal_rs::io::{ACCESS_DENIED, ALREADY_EXISTS, IS_DIRECTORY, NAME_TOO_LONG, NOT_DIRECTORY, NOT_EMPTY, WOULD_BLOCK};
    use dotnet_pal_rs::runtime::{BUFFER_TOO_SMALL, NOT_FOUND};
    use dotnet_pal_rs::{INVALID_ARGUMENT, OK};
    use std::{ffi::c_void, mem::size_of, path::{Path, PathBuf}, ptr};

    type PathCall = unsafe extern "C" fn(*const u8, usize) -> u32;
    /// Removes the scratch tree even when an assertion fails first.
    struct Scratch(PathBuf);
    impl Drop for Scratch { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }
    fn ops() -> &'static files::Ops { &api().files }
    fn bytes(path: &Path) -> &[u8] { path.to_str().unwrap().as_bytes() }
    fn open(path: &Path, flags: u32, mode: u32) -> (u32, *mut c_void) {
        let (mut handle, path) = (ptr::null_mut(), bytes(path));
        (unsafe { ops().open.unwrap()(path.as_ptr(), path.len(), flags, mode, &mut handle) }, handle)
    }
    fn close(file: *mut c_void) -> u32 { unsafe { ops().close.unwrap()(file) } }
    fn write_at(file: *mut c_void, offset: u64, data: &[u8]) -> (u32, usize) {
        let mut done = usize::MAX;
        (unsafe { ops().write_at.unwrap()(file, offset, data.as_ptr(), data.len(), &mut done) }, done)
    }
    fn read_at(file: *mut c_void, offset: u64, buffer: &mut [u8]) -> (u32, usize) {
        let mut got = usize::MAX;
        (unsafe { ops().read_at.unwrap()(file, offset, buffer.as_mut_ptr(), buffer.len(), &mut got) }, got)
    }
    fn status(file: *mut c_void) -> (u32, Status) {
        let mut out = Status { kind: 9, ..Status::default() };
        (unsafe { ops().status.unwrap()(file, &mut out, size_of::<Status>()) }, out)
    }
    fn path_status(path: &Path, follow: u32) -> (u32, Status) {
        let (mut out, path) = (Status { kind: 9, ..Status::default() }, bytes(path));
        (unsafe { ops().path_status.unwrap()(path.as_ptr(), path.len(), follow, &mut out, size_of::<Status>()) }, out)
    }
    fn at(call: Option<PathCall>, path: &Path) -> u32 { let path = bytes(path); unsafe { call.unwrap()(path.as_ptr(), path.len()) } }
    fn rename(from: &Path, to: &Path) -> u32 {
        let (from, to) = (bytes(from), bytes(to));
        unsafe { ops().rename.unwrap()(from.as_ptr(), from.len(), to.as_ptr(), to.len()) }
    }
    fn directory_create(path: &Path, mode: u32) -> u32 { let path = bytes(path); unsafe { ops().directory_create.unwrap()(path.as_ptr(), path.len(), mode) } }
    fn directory_open(path: &Path) -> (u32, *mut c_void) {
        let (mut handle, path) = (ptr::null_mut(), bytes(path));
        (unsafe { ops().directory_open.unwrap()(path.as_ptr(), path.len(), &mut handle) }, handle)
    }
    fn directory_read(directory: *mut c_void, name: &mut [u8]) -> (u32, usize, u32) {
        let (mut length, mut kind) = (usize::MAX, u32::MAX);
        (unsafe { ops().directory_read.unwrap()(directory, name.as_mut_ptr(), name.len(), &mut length, &mut kind) }, length, kind)
    }
    fn stats() -> files::Stats {
        let mut out = files::Stats::default();
        assert_eq!(unsafe { ops().read_stats.unwrap()(&mut out, size_of::<files::Stats>()) }, OK);
        out
    }
    fn ns(time: std::io::Result<std::time::SystemTime>) -> u64 { time.unwrap().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos() as u64 }

    #[test]
    fn files_transfer_describe_rename_and_enumerate() {
        assert_eq!(api().header.capabilities & files::CAP, files::CAP);
        let f = ops();
        let before = stats();
        let unique = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let root = std::env::temp_dir().join(format!("dotnet-pal-std-files-{}-{unique}", std::process::id()));
        let _cleanup = Scratch(root.clone());
        assert_eq!(directory_create(&root, 0o755), OK);
        assert_eq!(directory_create(&root, 0o755), ALREADY_EXISTS);
        assert_eq!(directory_create(&root.join("missing").join("deeper"), 0o755), NOT_FOUND, "never recursive");

        let data = root.join("data.bin");
        let (code, file) = open(&data, READ | WRITE | CREATE | EXCLUSIVE, 0o644);
        assert_eq!(code, OK);
        assert!(!file.is_null());
        assert_eq!(open(&data, READ | WRITE | CREATE | EXCLUSIVE, 0o644), (ALREADY_EXISTS, ptr::null_mut()));
        assert_eq!(open(&data, READ | CREATE | EXCLUSIVE, 0o644).0, ALREADY_EXISTS);
        assert_eq!(write_at(file, 0, b"hello world"), (OK, 11));
        let mut buffer = [0xffu8; 128];
        assert_eq!(read_at(file, 6, &mut buffer[..5]), (OK, 5));
        assert_eq!(&buffer[..5], b"world");
        assert_eq!(write_at(file, 100, b"tail"), (OK, 4), "a write past the end extends the file");
        assert_eq!(read_at(file, 0, &mut buffer), (OK, 104));
        assert_eq!(&buffer[..11], b"hello world");
        assert!(buffer[11..100].iter().all(|b| *b == 0), "the gap reads as zeros");
        assert_eq!(&buffer[100..104], b"tail");
        assert_eq!(read_at(file, 104, &mut buffer), (OK, 0), "end of file is zero bytes, not an error");
        assert_eq!(read_at(file, 1 << 40, &mut buffer), (OK, 0));
        assert_eq!(read_at(file, 0, &mut []), (OK, 0));
        assert_eq!(write_at(file, 0, &[]), (OK, 0));
        assert_eq!(unsafe { f.set_size.unwrap()(file, 5) }, OK);
        assert_eq!(read_at(file, 0, &mut buffer), (OK, 5));
        assert_eq!(unsafe { f.set_size.unwrap()(file, 4096) }, OK);
        assert_eq!(read_at(file, 4000, &mut buffer), (OK, 96));
        assert!(buffer[..96].iter().all(|b| *b == 0), "growth is zero-filled");
        assert_eq!(unsafe { f.flush.unwrap()(file) }, OK);

        let (code, described) = status(file);
        let meta = std::fs::metadata(&data).unwrap();
        assert_eq!((code, described.kind, described.size), (OK, NODE_FILE, 4096));
        assert_eq!((described.size, described.modified_ns), (meta.len(), ns(meta.modified())));
        assert!(described.modified_ns > 946_684_800_000_000_000);
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!((described.identity, described.device, described.mode), (meta.ino(), meta.dev(), meta.mode() & 0o7777));
            assert_eq!(described.mode & !0o644, 0, "the creation mode, less the umask");
            assert!(described.identity != 0 && described.changed_ns >= described.modified_ns);
        }
        #[cfg(not(unix))]
        assert_eq!((described.identity, described.device, described.mode, described.changed_ns), (0, 0, 0, 0));
        let (code, by_path) = path_status(&data, 1);
        assert_eq!((code, Status { accessed_ns: 0, ..by_path }), (OK, Status { accessed_ns: 0, ..described }));
        let sub = root.join("sub");
        assert_eq!(directory_create(&sub, 0o755), OK);
        assert_eq!(path_status(&sub, 0).1.kind, NODE_DIRECTORY);
        #[cfg(unix)]
        {
            let (link, dangling) = (root.join("link"), root.join("dangling"));
            std::os::unix::fs::symlink("data.bin", &link).unwrap();
            std::os::unix::fs::symlink("missing", &dangling).unwrap();
            let (code, followed) = path_status(&link, 1);
            assert_eq!((code, followed.kind, followed.identity, followed.size), (OK, NODE_FILE, described.identity, 4096));
            let (code, unfollowed) = path_status(&link, 0);
            assert_eq!((code, unfollowed.kind), (OK, files::NODE_SYMLINK));
            assert_ne!(unfollowed.identity, described.identity);
            assert_eq!(path_status(&dangling, 1), (NOT_FOUND, Status::default()));
            assert_eq!(path_status(&dangling, 0).1.kind, files::NODE_SYMLINK);
        }

        let missing = root.join("missing");
        assert_eq!(open(&missing, READ, 0), (NOT_FOUND, ptr::null_mut()));
        assert_eq!(open(&missing, WRITE | TRUNCATE, 0).0, NOT_FOUND, "no CREATE, no file");
        assert_eq!(path_status(&missing, 1), (NOT_FOUND, Status::default()));
        assert_eq!(at(f.remove, &missing), NOT_FOUND);
        assert_eq!(at(f.directory_remove, &missing), NOT_FOUND);
        assert_eq!(rename(&missing, &data), NOT_FOUND);
        assert_eq!(directory_open(&missing), (NOT_FOUND, ptr::null_mut()));
        for flags in [READ, WRITE, READ | WRITE] { assert_eq!(open(&sub, flags, 0), (IS_DIRECTORY, ptr::null_mut()), "flags {flags}"); }
        assert_eq!(at(f.remove, &sub), IS_DIRECTORY);
        assert!(sub.is_dir());
        assert_eq!(open(&data.join("x"), READ, 0).0, NOT_DIRECTORY);
        assert_eq!(directory_create(&data.join("x"), 0o755), NOT_DIRECTORY);
        assert_eq!(directory_open(&data).0, NOT_DIRECTORY);
        assert_eq!(at(f.directory_remove, &data), NOT_DIRECTORY);
        assert_eq!(at(f.directory_remove, &root), NOT_EMPTY);

        let (first, second) = (root.join("a.txt"), root.join("b.txt"));
        for (path, text) in [(&first, &b"first"[..]), (&second, &b"second"[..])] {
            let (code, handle) = open(path, WRITE | CREATE, 0o600);
            assert_eq!(code, OK);
            assert_eq!(write_at(handle, 0, text), (OK, text.len()));
            assert_eq!(close(handle), OK);
        }
        assert_eq!(rename(&first, &second), OK, "rename replaces an existing file");
        assert_eq!(path_status(&first, 0).0, NOT_FOUND);
        assert_eq!(std::fs::read(&second).unwrap(), b"first");
        let (code, handle) = open(&second, WRITE | TRUNCATE, 0);
        assert_eq!((code, status(handle).1.size), (OK, 0));
        assert_eq!(read_at(handle, 0, &mut buffer).0, ACCESS_DENIED, "a write-only handle does not read");
        assert_eq!(close(handle), OK);
        let created = root.join("read-created");
        let (code, handle) = open(&created, READ | CREATE, 0o600);
        assert_eq!(code, OK, "creating a file needs no write access");
        assert_eq!(write_at(handle, 0, b"x").0, ACCESS_DENIED, "a read-only handle does not write");
        assert_eq!((read_at(handle, 0, &mut buffer), close(handle)), ((OK, 0), OK));
        // 270 bytes: APFS and NTFS take the name, most Linux filesystems refuse it. Either way it is never listed.
        let long = root.join("\u{3042}".repeat(90));
        let (code, handle) = open(&long, WRITE | CREATE, 0o600);
        if code == OK { assert_eq!(close(handle), OK); } else { assert_eq!(code, NAME_TOO_LONG); }

        let (code, directory) = directory_open(&root);
        assert_eq!(code, OK);
        let (mut listed, mut name) = (Vec::new(), [0xffu8; 300]);
        loop {
            let (code, length, kind) = directory_read(directory, &mut name);
            if code == NOT_FOUND { break; }
            assert_eq!(code, OK);
            assert!(name[length..].iter().all(|b| *b == 0));
            listed.push((String::from_utf8(name[..length].to_vec()).unwrap(), kind));
        }
        assert_eq!(directory_read(directory, &mut name), (NOT_FOUND, 0, 0), "the end is sticky");
        assert_eq!(directory_read(directory, &mut name[..254]).0, INVALID_ARGUMENT);
        assert_eq!(unsafe { f.directory_close.unwrap()(directory) }, OK);
        listed.sort();
        let mut expected = vec![("b.txt", NODE_FILE), ("data.bin", NODE_FILE), ("read-created", NODE_FILE), ("sub", NODE_DIRECTORY)];
        if cfg!(unix) { expected.extend([("dangling", files::NODE_SYMLINK), ("link", files::NODE_SYMLINK)]); }
        expected.sort();
        assert_eq!(listed.iter().map(|(name, kind)| (name.as_str(), *kind)).collect::<Vec<_>>(), expected, "exactly the created names, never . or ..");
        let (code, directory) = directory_open(&sub);
        assert_eq!((code, directory_read(directory, &mut name)), (OK, (NOT_FOUND, 0, 0)), "an empty directory lists nothing");
        assert_eq!(unsafe { f.directory_close.unwrap()(directory) }, OK);

        let current = std::env::current_dir().unwrap();
        let mut needed = 0usize;
        assert_eq!(unsafe { f.current_directory.unwrap()(ptr::null_mut(), 0, &mut needed) }, BUFFER_TOO_SMALL);
        assert_eq!(needed, bytes(&current).len() + 1, "needed counts the NUL");
        let mut text = vec![0xffu8; needed];
        assert_eq!(unsafe { f.current_directory.unwrap()(text.as_mut_ptr(), needed - 1, &mut needed) }, BUFFER_TOO_SMALL);
        assert_eq!(needed, text.len());
        assert!(text[..needed - 1].iter().all(|b| *b == 0), "a short buffer is cleared, not half filled");
        assert_eq!(unsafe { f.current_directory.unwrap()(text.as_mut_ptr(), text.len(), &mut needed) }, OK);
        assert_eq!((needed, &text[..needed - 1], text[needed - 1]), (text.len(), bytes(&current), 0));

        assert_eq!(close(file), OK);
        let mut leftovers = vec![data, second, created];
        if cfg!(unix) { leftovers.extend([root.join("link"), root.join("dangling")]); }
        if long.exists() { leftovers.push(long); }
        for path in &leftovers { assert_eq!(at(f.remove, path), OK, "{path:?}"); }
        assert_eq!(at(f.directory_remove, &sub), OK);
        assert_eq!(at(f.directory_remove, &root), OK);
        assert!(!root.exists());
        let after = stats();
        for (name, was, now) in [("open", before.open_ok, after.open_ok), ("close", before.close_ok, after.close_ok), ("read", before.read_ok, after.read_ok),
            ("write", before.write_ok, after.write_ok), ("size", before.size_ok, after.size_ok), ("flush", before.flush_ok, after.flush_ok),
            ("status", before.status_ok, after.status_ok), ("remove", before.remove_ok, after.remove_ok), ("rename", before.rename_ok, after.rename_ok),
            ("directory", before.directory_ok, after.directory_ok), ("failed", before.rejected_or_failed, after.rejected_or_failed)] {
            assert!(now > was, "{name} counter did not move: {was} -> {now}");
        }
    }

    #[test]
    fn files_reject_malformed_arguments_before_the_provider() {
        let f = ops();
        let before = stats().rejected_or_failed;
        let path = std::env::temp_dir().join("dotnet-pal-std-files-never-created");
        let text = bytes(&path);
        let mut handle = ptr::null_mut();
        assert_eq!(unsafe { f.open.unwrap()(text.as_ptr(), 0, READ, 0, &mut handle) }, INVALID_ARGUMENT, "empty path");
        assert_eq!(unsafe { f.open.unwrap()(ptr::null(), 4, READ, 0, &mut handle) }, INVALID_ARGUMENT);
        assert_eq!(unsafe { f.open.unwrap()(b"a\0b".as_ptr(), 3, READ | WRITE | CREATE, 0o600, &mut handle) }, INVALID_ARGUMENT, "embedded NUL");
        assert_eq!(unsafe { f.open.unwrap()(text.as_ptr(), text.len(), READ, 0, ptr::null_mut()) }, INVALID_ARGUMENT);
        let long = [b'a'; dotnet_pal_rs::runtime::MAX_NAME + 1];
        assert_eq!(unsafe { f.open.unwrap()(long.as_ptr(), long.len(), READ, 0, &mut handle) }, INVALID_ARGUMENT);
        for flags in [0, CREATE, TRUNCATE, READ | EXCLUSIVE, READ | TRUNCATE, READ | 32] { assert_eq!(open(&path, flags, 0), (INVALID_ARGUMENT, ptr::null_mut()), "flags {flags}"); }
        assert_eq!(open(&path, READ | WRITE | CREATE, 0o10000).0, INVALID_ARGUMENT);
        assert_eq!(directory_create(&path, 0o10000), INVALID_ARGUMENT);
        assert!(!path.exists());
        assert_eq!(unsafe { f.remove.unwrap()(text.as_ptr(), 0) }, INVALID_ARGUMENT);
        assert_eq!(unsafe { f.rename.unwrap()(text.as_ptr(), text.len(), b"to\0there".as_ptr(), 8) }, INVALID_ARGUMENT);
        assert_eq!(unsafe { f.directory_remove.unwrap()(ptr::null(), 1) }, INVALID_ARGUMENT);
        assert_eq!(unsafe { f.directory_open.unwrap()(text.as_ptr(), text.len(), ptr::null_mut()) }, INVALID_ARGUMENT);
        assert_eq!(path_status(&path, 2), (INVALID_ARGUMENT, Status::default()));
        let mut described = Status::default();
        assert_eq!(unsafe { f.path_status.unwrap()(text.as_ptr(), text.len(), 1, &mut described, size_of::<Status>() - 1) }, INVALID_ARGUMENT);

        let null = ptr::null_mut();
        let mut buffer = [0u8; 300];
        assert_eq!(close(null), INVALID_ARGUMENT);
        assert_eq!(read_at(null, 0, &mut buffer), (INVALID_ARGUMENT, 0));
        assert_eq!(write_at(null, 0, &buffer), (INVALID_ARGUMENT, 0));
        assert_eq!(unsafe { f.set_size.unwrap()(null, 0) }, INVALID_ARGUMENT);
        assert_eq!(unsafe { f.flush.unwrap()(null) }, INVALID_ARGUMENT);
        assert_eq!(status(null), (INVALID_ARGUMENT, Status::default()));
        assert_eq!(directory_read(null, &mut buffer), (INVALID_ARGUMENT, 0, 0));
        assert_eq!(unsafe { f.directory_close.unwrap()(null) }, INVALID_ARGUMENT);
        // Ranges are checked before the handle is ever dereferenced.
        let bogus = &mut buffer as *mut _ as *mut c_void;
        assert_eq!(unsafe { f.read_at.unwrap()(bogus, i64::MAX as u64 + 1, buffer.as_mut_ptr(), 1, &mut 0) }, INVALID_ARGUMENT);
        assert_eq!(unsafe { f.write_at.unwrap()(bogus, i64::MAX as u64, buffer.as_ptr(), 1, &mut 0) }, INVALID_ARGUMENT);
        assert_eq!(unsafe { f.read_at.unwrap()(bogus, 0, ptr::null_mut(), 1, &mut 0) }, INVALID_ARGUMENT);
        assert_eq!(unsafe { f.read_at.unwrap()(bogus, 0, buffer.as_mut_ptr(), 1, ptr::null_mut()) }, INVALID_ARGUMENT);
        assert_eq!(unsafe { f.set_size.unwrap()(bogus, i64::MAX as u64 + 1) }, INVALID_ARGUMENT);
        assert_eq!(unsafe { f.current_directory.unwrap()(buffer.as_mut_ptr(), buffer.len(), ptr::null_mut()) }, INVALID_ARGUMENT);
        assert_eq!(unsafe { f.current_directory.unwrap()(ptr::null_mut(), 1, &mut 0) }, INVALID_ARGUMENT);
        assert_eq!(unsafe { f.read_stats.unwrap()(ptr::null_mut(), size_of::<files::Stats>()) }, INVALID_ARGUMENT);
        assert!(stats().rejected_or_failed >= before + 30);
    }

    type TextCall = unsafe extern "C" fn(*const u8, usize, *mut u8, usize, *mut usize) -> u32;
    type PairCall = unsafe extern "C" fn(*const u8, usize, *const u8, usize) -> u32;
    const KEEP: u64 = files::TIME_KEEP;
    const END: u64 = i64::MAX as u64;
    fn pair(call: Option<PairCall>, first: &[u8], second: &Path) -> u32 {
        let second = bytes(second);
        unsafe { call.unwrap()(first.as_ptr(), first.len(), second.as_ptr(), second.len()) }
    }
    /// Status, needed length and the whole buffer of a call that answers with text; no buffer at all for capacity 0.
    fn text(call: Option<TextCall>, path: &Path, capacity: usize) -> (u32, usize, Vec<u8>) {
        let (mut needed, mut out, path) = (usize::MAX, vec![0xffu8; capacity], bytes(path));
        let code = unsafe { call.unwrap()(path.as_ptr(), path.len(), if capacity == 0 { ptr::null_mut() } else { out.as_mut_ptr() }, capacity, &mut needed) };
        (code, needed, out)
    }
    /// The text contract: the text and its NUL in a buffer that holds them, the length needed and a cleared buffer in one that does not.
    fn assert_text(call: Option<TextCall>, path: &Path, expected: &[u8]) {
        let size = expected.len() + 1;
        let (code, needed, out) = text(call, path, size + 8);
        assert_eq!((code, needed, &out[..size - 1], &out[size - 1..]), (OK, size, expected, &[0u8; 9][..]), "{path:?}");
        assert_eq!(text(call, path, size).0, OK);
        assert_eq!(text(call, path, size - 1), (BUFFER_TOO_SMALL, size, vec![0u8; size - 1]));
        assert_eq!(text(call, path, 0), (BUFFER_TOO_SMALL, size, Vec::new()));
    }
    fn set_mode(path: &Path, mode: u32) -> u32 { let path = bytes(path); unsafe { ops().set_mode.unwrap()(path.as_ptr(), path.len(), mode) } }
    fn set_times(path: &Path, follow: u32, accessed_ns: u64, modified_ns: u64) -> u32 {
        let path = bytes(path);
        unsafe { ops().set_times.unwrap()(path.as_ptr(), path.len(), follow, accessed_ns, modified_ns) }
    }
    /// Access and modification time as the OS reports them for the path itself.
    fn times(path: &Path) -> (u64, u64) { let meta = std::fs::symlink_metadata(path).unwrap(); (ns(meta.accessed()), ns(meta.modified())) }
    fn lock(file: *mut c_void, mode: u32, wait: u32) -> u32 { unsafe { ops().lock.unwrap()(file, mode, wait) } }
    fn lock_range(file: *mut c_void, offset: u64, length: u64, mode: u32) -> u32 { unsafe { ops().lock_range.unwrap()(file, offset, length, mode) } }

    #[test]
    fn files_change_attributes_link_and_lock() {
        let f = ops();
        let before = stats();
        let unique = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let root = std::env::temp_dir().join(format!("dotnet-pal-std-files-more-{}-{unique}", std::process::id()));
        let _cleanup = Scratch(root.clone());
        assert_eq!((directory_create(&root, 0o755), directory_create(&root.join("sub"), 0o755)), (OK, OK));
        let (data, missing) = (root.join("data.bin"), root.join("missing"));
        let (code, file) = open(&data, READ | WRITE | CREATE | EXCLUSIVE, 0o644);
        assert_eq!(code, OK);

        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let mode = |path: &Path| std::fs::metadata(path).unwrap().mode() & 0o7777;
            assert_eq!((set_mode(&data, 0o751), mode(&data)), (OK, 0o751));
            assert_eq!((unsafe { f.set_file_mode.unwrap()(file, 0o604) }, mode(&data), status(file).1.mode), (OK, 0o604, 0o604));
            let link = root.join("link");
            std::os::unix::fs::symlink("data.bin", &link).unwrap();
            assert_eq!((set_mode(&link, 0o640), mode(&data)), (OK, 0o640), "a path that is a link names the file behind it");
            assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        }
        #[cfg(not(unix))]
        {
            let read_only = |path: &Path| std::fs::metadata(path).unwrap().permissions().readonly();
            assert_eq!((set_mode(&data, 0o444), read_only(&data)), (OK, true), "the one permission bit Windows has");
            assert_eq!((unsafe { f.set_file_mode.unwrap()(file, 0o644) }, read_only(&data)), (OK, false));
        }
        assert_eq!(set_mode(&missing, 0o600), NOT_FOUND);

        // NTFS keeps 100 ns; the Unix filesystems keep every nanosecond.
        let odd = if cfg!(unix) { 89 } else { 0 };
        let (t1, t2, t3) = (1_234_567_890_123_456_700 + odd, 987_654_321_000_000_100 + odd, 1_500_000_000_999_999_900 + odd);
        assert_eq!((set_times(&data, 1, t1, t2), times(&data)), (OK, (t1, t2)));
        assert_eq!((set_times(&data, 1, KEEP, t3), times(&data)), (OK, (t1, t3)), "TIME_KEEP leaves the access time");
        assert_eq!((set_times(&data, 1, t2, KEEP), times(&data)), (OK, (t2, t3)), "TIME_KEEP leaves the modification time");
        assert_eq!((set_times(&data, 1, KEEP, KEEP), times(&data)), (OK, (t2, t3)));
        assert_eq!((set_times(&data, 0, t1, t1), times(&data)), (OK, (t1, t1)), "not a link: nothing to follow");
        assert_eq!((unsafe { f.set_file_times.unwrap()(file, t3, KEEP) }, times(&data)), (OK, (t3, t1)));
        assert_eq!((unsafe { f.set_file_times.unwrap()(file, KEEP, t2) }, times(&data)), (OK, (t3, t2)));
        let (code, described) = status(file);
        assert_eq!((code, described.accessed_ns, described.modified_ns), (OK, t3, t2));
        assert_eq!(set_times(&missing, 1, t1, KEEP), NOT_FOUND);
        #[cfg(unix)]
        {
            let link = root.join("link");
            assert_eq!((set_times(&link, 0, t1, t1), times(&link), times(&data)), (OK, (t1, t1), (t3, t2)), "follow_links = 0 names the link itself");
            assert_eq!((set_times(&link, 1, t2, t3), times(&data)), (OK, (t2, t3)));
        }
        assert_eq!(set_times(&data, 1, END, KEEP), OK, "the last time the boundary takes");
        assert_eq!(unsafe { f.set_file_times.unwrap()(file, KEEP, END) }, OK);

        let hard = root.join("hard");
        assert_eq!(write_at(file, 0, b"0123456789"), (OK, 10));
        assert_eq!(pair(f.link, bytes(&data), &hard), OK);
        assert_eq!(std::fs::read(&hard).unwrap(), b"0123456789");
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let (first, second) = (std::fs::metadata(&data).unwrap(), std::fs::metadata(&hard).unwrap());
            assert_eq!((first.ino(), first.dev(), first.nlink()), (second.ino(), second.dev(), 2), "one node, two names");
        }
        assert_eq!(pair(f.link, bytes(&data), &hard), ALREADY_EXISTS);
        assert_eq!(pair(f.link, bytes(&missing), &root.join("hard2")), NOT_FOUND);
        assert!(!root.join("hard2").exists());

        let canonical = std::fs::canonicalize(&root).unwrap();
        assert_text(f.real_path, &root.join("sub").join("..").join("data.bin"), bytes(&canonical.join("data.bin")));
        assert_text(f.real_path, &root, bytes(&canonical));
        assert_eq!(text(f.real_path, &missing, 64), (NOT_FOUND, 0, vec![0u8; 64]));
        // Windows reserves symbolic links for privileged accounts.
        #[cfg(unix)]
        {
            let (link, dangling, long, target300) = (root.join("s"), root.join("s-dangling"), root.join("s-300"), [b'x'; 300]);
            for (target, link) in [(&b"data.bin"[..], &link), (&b"no/../such//target/"[..], &dangling), (&target300[..], &long)] {
                assert_eq!(pair(f.symlink, target, link), OK);
                assert_eq!(bytes(&std::fs::read_link(link).unwrap()), target, "the target text is stored as given");
                assert_text(f.read_link, link, target);
            }
            assert_eq!((std::fs::read(&link).unwrap(), path_status(&dangling, 0).1.kind), (b"0123456789".to_vec(), files::NODE_SYMLINK));
            assert_eq!(pair(f.symlink, b"data.bin", &link), ALREADY_EXISTS);
            assert_eq!(text(f.read_link, &data, 64), (INVALID_ARGUMENT, 0, vec![0u8; 64]), "not a link");
            assert_eq!(text(f.read_link, &root, 64).0, INVALID_ARGUMENT);
            assert_eq!(text(f.read_link, &missing, 64), (NOT_FOUND, 0, vec![0u8; 64]));
            assert_text(f.real_path, &root.join("sub").join("..").join("s"), bytes(&canonical.join("data.bin")));
            assert_eq!(text(f.real_path, &dangling, 64).0, NOT_FOUND);
        }

        // Whole-file locks are held by the handle. The OS's side of each answer is a file std opened itself.
        let path = root.join("locked");
        std::fs::write(&path, b"0123456789").unwrap();
        let handle = |flags: u32| { let (code, handle) = open(&path, flags, 0); assert_eq!(code, OK); handle };
        let (l1, l2, l3, probe) = (handle(READ | WRITE), handle(READ | WRITE), handle(READ), std::fs::File::open(&path).unwrap());
        assert_eq!((lock(l1, LOCK_SHARED, 0), lock(l2, LOCK_SHARED, 0)), (OK, OK), "shared locks coexist");
        assert!(probe.try_lock_shared().is_ok() && probe.unlock().is_ok());
        assert_eq!(lock(l3, LOCK_EXCLUSIVE, 0), WOULD_BLOCK);
        assert!(matches!(probe.try_lock(), Err(std::fs::TryLockError::WouldBlock)));
        assert_eq!((lock(l1, LOCK_UNLOCK, 0), lock(l3, LOCK_EXCLUSIVE, 0)), (OK, WOULD_BLOCK), "one of the two is left");
        assert_eq!((lock(l2, LOCK_UNLOCK, 0), lock(l3, LOCK_EXCLUSIVE, 0)), (OK, OK), "UNLOCK releases");
        assert_eq!((lock(l1, LOCK_SHARED, 0), lock(l2, LOCK_EXCLUSIVE, 0)), (WOULD_BLOCK, WOULD_BLOCK));
        assert!(matches!(probe.try_lock_shared(), Err(std::fs::TryLockError::WouldBlock)));
        assert_eq!((lock(l2, LOCK_UNLOCK, 0), lock(l1, LOCK_SHARED, 0)), (OK, WOULD_BLOCK), "UNLOCK without a lock takes nobody else's");
        assert_eq!((close(l3), lock(l1, LOCK_EXCLUSIVE, 0)), (OK, OK), "close releases");
        let finished = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let waiter = {
            let (finished, waiting) = (finished.clone(), l2 as usize);
            std::thread::spawn(move || { let code = lock(waiting as *mut c_void, LOCK_EXCLUSIVE, 1); finished.store(true, std::sync::atomic::Ordering::SeqCst); code })
        };
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(!finished.load(std::sync::atomic::Ordering::SeqCst), "wait = 1 waits for the holder");
        assert_eq!((lock(l1, LOCK_UNLOCK, 0), waiter.join().unwrap()), (OK, OK), "and proceeds when the holder unlocks");
        assert_eq!((lock(l1, LOCK_SHARED, 0), lock(l2, LOCK_UNLOCK, 1), lock(l1, LOCK_SHARED, 1), lock(l1, LOCK_UNLOCK, 0)), (WOULD_BLOCK, OK, OK, OK));

        #[cfg(any(target_os = "linux", target_vendor = "apple"))]
        {
            let l3 = handle(READ | WRITE);
            assert_eq!((lock_range(l1, 0, 10, LOCK_EXCLUSIVE), lock_range(l2, 10, 10, LOCK_EXCLUSIVE)), (OK, OK), "disjoint ranges");
            assert_eq!((lock_range(l3, 9, 1, LOCK_SHARED), lock_range(l3, 5, 10, LOCK_EXCLUSIVE), lock_range(l3, 19, 5, LOCK_SHARED)), (WOULD_BLOCK, WOULD_BLOCK, WOULD_BLOCK), "overlapping ones");
            assert_eq!((lock_range(l3, 20, 5, LOCK_EXCLUSIVE), lock_range(l3, 20, 5, LOCK_UNLOCK), lock_range(l1, 0, 10, LOCK_UNLOCK), lock_range(l2, 10, 10, LOCK_UNLOCK)), (OK, OK, OK, OK));
            assert_eq!((lock_range(l1, 0, 10, LOCK_SHARED), lock_range(l2, 5, 10, LOCK_SHARED)), (OK, OK), "shared ranges coexist");
            assert_eq!((lock_range(l3, 12, 1, LOCK_EXCLUSIVE), lock_range(l3, 7, 1, LOCK_EXCLUSIVE), lock_range(l3, 15, 1, LOCK_EXCLUSIVE)), (WOULD_BLOCK, WOULD_BLOCK, OK), "and exclude an exclusive one");
            assert_eq!((lock_range(l1, END - 1, 1, LOCK_EXCLUSIVE), lock_range(l3, END - 1, 1, LOCK_SHARED)), (OK, WOULD_BLOCK), "the last byte there is");
            assert_eq!((close(l1), lock_range(l3, 0, 5, LOCK_EXCLUSIVE), lock_range(l3, END - 1, 1, LOCK_EXCLUSIVE), lock_range(l3, 5, 1, LOCK_EXCLUSIVE)), (OK, OK, OK, WOULD_BLOCK), "close releases");
            assert_eq!((close(l2), lock_range(l3, 0, END, LOCK_EXCLUSIVE), lock_range(l3, 0, END, LOCK_UNLOCK)), (OK, OK, OK));
            // The OS wants read access for a shared range and write access for an exclusive one.
            let (reading, writing) = (handle(READ), handle(WRITE));
            assert_eq!((lock_range(reading, 100, 1, LOCK_EXCLUSIVE), lock_range(writing, 100, 1, LOCK_SHARED)), (ACCESS_DENIED, ACCESS_DENIED));
            assert_eq!((lock_range(reading, 100, 1, LOCK_SHARED), lock_range(writing, 101, 1, LOCK_EXCLUSIVE), lock_range(l3, 100, 2, LOCK_EXCLUSIVE)), (OK, OK, WOULD_BLOCK));
            assert_eq!((close(reading), close(writing), lock_range(l3, 100, 2, LOCK_EXCLUSIVE), close(l3)), (OK, OK, OK, OK));
        }
        #[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
        assert_eq!((lock_range(l1, 0, 10, LOCK_EXCLUSIVE), close(l1), close(l2)), (dotnet_pal_rs::UNSUPPORTED, OK, OK), "no range lock here that the handle holds");

        assert_eq!(close(file), OK);
        let after = stats();
        for (name, was, now) in [("attribute", before.attribute_ok, after.attribute_ok), ("link", before.link_ok, after.link_ok), ("lock", before.lock_ok, after.lock_ok),
            ("failed", before.rejected_or_failed, after.rejected_or_failed)] {
            assert!(now > was, "{name} counter did not move: {was} -> {now}");
        }
    }

    #[test]
    fn files_reject_malformed_attribute_link_and_lock_arguments() {
        let f = ops();
        let before = stats().rejected_or_failed;
        let path = std::env::temp_dir().join("dotnet-pal-std-files-never-linked");
        let name = bytes(&path);
        assert_eq!(set_mode(&path, 0o10000), INVALID_ARGUMENT);
        assert_eq!(unsafe { f.set_mode.unwrap()(ptr::null(), 4, 0o600) }, INVALID_ARGUMENT);
        assert_eq!(unsafe { f.set_mode.unwrap()(name.as_ptr(), 0, 0o600) }, INVALID_ARGUMENT, "empty path");
        for (accessed_ns, modified_ns) in [(END + 1, KEEP), (KEEP, END + 1), (KEEP - 1, 0)] { assert_eq!(set_times(&path, 1, accessed_ns, modified_ns), INVALID_ARGUMENT, "past the range and not TIME_KEEP"); }
        assert_eq!(set_times(&path, 2, 0, 0), INVALID_ARGUMENT);
        assert_eq!(unsafe { f.set_times.unwrap()(ptr::null(), 4, 1, 0, 0) }, INVALID_ARGUMENT);
        let long = [b'a'; dotnet_pal_rs::runtime::MAX_NAME + 1];
        for call in [f.link, f.symlink] {
            for first in [&b""[..], &b"a\0b"[..], &long[..]] { assert_eq!(pair(call, first, &path), INVALID_ARGUMENT); }
            assert_eq!(unsafe { call.unwrap()(ptr::null(), 4, name.as_ptr(), name.len()) }, INVALID_ARGUMENT);
            assert_eq!(unsafe { call.unwrap()(name.as_ptr(), name.len(), ptr::null(), 4) }, INVALID_ARGUMENT);
            assert_eq!(unsafe { call.unwrap()(name.as_ptr(), name.len(), b"to\0there".as_ptr(), 8) }, INVALID_ARGUMENT);
        }
        assert!(std::fs::symlink_metadata(&path).is_err());
        let mut buffer = [0u8; 300];
        for call in [f.read_link, f.real_path] {
            let mut needed = 9usize;
            assert_eq!((unsafe { call.unwrap()(ptr::null(), 4, buffer.as_mut_ptr(), buffer.len(), &mut needed) }, needed), (INVALID_ARGUMENT, 0));
            assert_eq!(unsafe { call.unwrap()(name.as_ptr(), 0, buffer.as_mut_ptr(), buffer.len(), &mut needed) }, INVALID_ARGUMENT);
            assert_eq!(unsafe { call.unwrap()(name.as_ptr(), name.len(), ptr::null_mut(), 8, &mut needed) }, INVALID_ARGUMENT);
            assert_eq!(unsafe { call.unwrap()(name.as_ptr(), name.len(), buffer.as_mut_ptr(), buffer.len(), ptr::null_mut()) }, INVALID_ARGUMENT);
        }
        // Checked before the handle is ever dereferenced.
        let (null, bogus) = (ptr::null_mut(), &mut buffer as *mut _ as *mut c_void);
        assert_eq!(unsafe { f.set_file_mode.unwrap()(null, 0o600) }, INVALID_ARGUMENT);
        assert_eq!(unsafe { f.set_file_mode.unwrap()(bogus, 0o10000) }, INVALID_ARGUMENT);
        assert_eq!(unsafe { f.set_file_times.unwrap()(null, 0, 0) }, INVALID_ARGUMENT);
        assert_eq!(unsafe { f.set_file_times.unwrap()(bogus, END + 1, KEEP) }, INVALID_ARGUMENT);
        assert_eq!(unsafe { f.set_file_times.unwrap()(bogus, KEEP, END + 1) }, INVALID_ARGUMENT);
        assert_eq!(lock(null, LOCK_SHARED, 0), INVALID_ARGUMENT);
        for (mode, wait) in [(0, 0), (4, 0), (LOCK_EXCLUSIVE, 2)] { assert_eq!(lock(bogus, mode, wait), INVALID_ARGUMENT, "mode {mode} wait {wait}"); }
        assert_eq!(lock_range(null, 0, 1, LOCK_SHARED), INVALID_ARGUMENT);
        for (offset, length, mode) in [(0, 1, 0), (0, 1, 4), (0, 0, LOCK_EXCLUSIVE), (END + 1, 1, LOCK_EXCLUSIVE), (END, 1, LOCK_EXCLUSIVE), (1, END, LOCK_EXCLUSIVE), (u64::MAX, u64::MAX, LOCK_UNLOCK)] {
            assert_eq!(lock_range(bogus, offset, length, mode), INVALID_ARGUMENT, "offset {offset} length {length} mode {mode}");
        }
        assert!(stats().rejected_or_failed >= before + 45);
    }
}

#[cfg(all(any(target_os = "linux", target_os = "macos"), any(target_arch = "aarch64", target_arch = "x86_64")))]
mod faults_table {
    use super::api;
    use dotnet_pal_rs::faults::{self, Frame, ACCESS, RESUME, UNHANDLED};
    use dotnet_pal_rs::{kernel::BUSY, INVALID_ARGUMENT, OK};
    use std::{ffi::c_void, mem::size_of, ptr, sync::atomic::{AtomicU32, AtomicUsize, Ordering::SeqCst}};

    // Assembly, so the faulting instructions have known addresses and lengths. `sym` spells the names the way the target's C ABI does.
    extern "C" {
        /// Loads through its argument in its first instruction; from `loaded` on it returns the sum of the result and second argument registers.
        fn pal_std_fault_load(address: usize) -> u64;
        fn pal_std_fault_loaded();
        /// Stamps caller-saved integer and vector registers, stores through its argument at `store`, and returns zero when every stamp survived.
        fn pal_std_fault_keeps(address: usize) -> u64;
        fn pal_std_fault_store();
        fn pal_std_fault_stored();
        /// Loads through an sp that is 8 off a 16-byte boundary at `skewed`, which AArch64 checks; `straight` puts sp back.
        #[cfg(target_arch = "aarch64")]
        fn pal_std_fault_skew() -> u64;
        #[cfg(target_arch = "aarch64")]
        fn pal_std_fault_skewed();
        #[cfg(target_arch = "aarch64")]
        fn pal_std_fault_straight();
    }
    /// A label inside a routine. Mach-O lets the linker split code at every global symbol that is not declared an alternate entry.
    #[cfg(target_vendor = "apple")]
    macro_rules! inner { ($label:literal) => { concat!(".globl {", $label, "}\n.alt_entry {", $label, "}\n{", $label, "}:") } }
    #[cfg(not(target_vendor = "apple"))]
    macro_rules! inner { ($label:literal) => { concat!(".globl {", $label, "}\n{", $label, "}:") } }
    #[cfg(target_arch = "aarch64")]
    core::arch::global_asm!(
        ".text", ".p2align 2",
        ".globl {load}", "{load}:", "ldr x0, [x0]",
        inner!("loaded"), "add x0, x0, x1", "ret",
        ".globl {keeps}", "{keeps}:",
        "mov x9, #0x1109", "mov x10, #0x110a", "mov x15, #0x110f",
        "mov x2, #0x2200", "fmov d0, x2", "add x2, x2, #7", "fmov d7, x2", "add x2, x2, #24", "fmov d31, x2",
        inner!("store"), "str x9, [x0]",
        inner!("stored"), "mov x0, #0",
        "mov x1, #0x1109", "eor x1, x1, x9", "orr x0, x0, x1",
        "mov x1, #0x110a", "eor x1, x1, x10", "orr x0, x0, x1",
        "mov x1, #0x110f", "eor x1, x1, x15", "orr x0, x0, x1",
        "mov x2, #0x2200", "fmov x1, d0", "eor x1, x1, x2", "orr x0, x0, x1",
        "add x2, x2, #7", "fmov x1, d7", "eor x1, x1, x2", "orr x0, x0, x1",
        "add x2, x2, #24", "fmov x1, d31", "eor x1, x1, x2", "orr x0, x0, x1",
        "ret",
        ".globl {skew}", "{skew}:", "mov x9, sp", "sub x10, x9, #8", "mov sp, x10",
        inner!("skewed"), "ldr x0, [sp]",
        inner!("straight"), "mov sp, x9", "ret",
        load = sym pal_std_fault_load, loaded = sym pal_std_fault_loaded, keeps = sym pal_std_fault_keeps, store = sym pal_std_fault_store, stored = sym pal_std_fault_stored,
        skew = sym pal_std_fault_skew, skewed = sym pal_std_fault_skewed, straight = sym pal_std_fault_straight,
    );
    #[cfg(target_arch = "x86_64")]
    core::arch::global_asm!(
        ".text",
        ".globl {load}", "{load}:", "mov rax, [rdi]",
        inner!("loaded"), "add rax, rdx", "ret",
        ".globl {keeps}", "{keeps}:",
        "mov r8, 0x1108", "mov r9, 0x1109", "mov r10, 0x110a", "mov r11, 0x110b",
        "mov eax, 0x2200", "movq xmm0, rax", "add rax, 7", "movq xmm15, rax",
        inner!("store"), "mov [rdi], r8",
        inner!("stored"), "xor eax, eax",
        "xor r8, 0x1108", "or rax, r8", "xor r9, 0x1109", "or rax, r9", "xor r10, 0x110a", "or rax, r10", "xor r11, 0x110b", "or rax, r11",
        "movq rcx, xmm0", "xor rcx, 0x2200", "or rax, rcx",
        "movq rcx, xmm15", "xor rcx, 0x2207", "or rax, rcx",
        "ret",
        load = sym pal_std_fault_load, loaded = sym pal_std_fault_loaded, keeps = sym pal_std_fault_keeps, store = sym pal_std_fault_store, stored = sym pal_std_fault_stored,
    );
    /// The frame's program counter, result register, second argument register and stack pointer.
    #[cfg(target_arch = "aarch64")]
    fn registers(frame: &mut Frame) -> (&mut u64, &mut u64, &mut u64, u64) { let Frame { x: [x0, x1, ..], sp, pc, .. } = frame; (pc, x0, x1, *sp) }
    /// rdx is the register where encoding order and alphabetical order part.
    #[cfg(target_arch = "x86_64")]
    fn registers(frame: &mut Frame) -> (&mut u64, &mut u64, &mut u64, u64) { let Frame { registers: [rax, _, rdx, _, rsp, ..], rip, .. } = frame; (rip, rax, rdx, *rsp) }

    /// Where the load faults and resumes, and where the store does.
    fn sites() -> [u64; 4] { [pal_std_fault_load as *const (), pal_std_fault_loaded as *const (), pal_std_fault_store as *const (), pal_std_fault_stored as *const ()].map(|site| site as u64) }

    #[cfg(target_arch = "aarch64")]
    fn skew_sites() -> [u64; 2] { [pal_std_fault_skewed as *const (), pal_std_fault_straight as *const ()].map(|site| site as u64) }
    fn seen() -> [usize; 6] { [0, 1, 2, 3, 4, 5].map(|i| SEEN[i].load(SeqCst)) }

    const MARKER: u64 = 0xfeed_face_cafe_0000;
    const SECOND: u64 = 0x0123_4567_0000_0000;
    const RECOVER: u32 = 0;
    const DECLINE: u32 = 1;
    const NEST: u32 = 2;
    const SENT: u32 = 3;
    const NAME: &str = "faults_table::faults_are_reported_resumed_and_end_the_process_when_declined";
    const CHILD: &str = "PAL_STD_FAULTS_CHILD";
    const CALLED: &[u8] = b"pal-std-faults-handler-called\n";
    static COOKIE: AtomicU32 = AtomicU32::new(0x1234);
    static MODE: AtomicU32 = AtomicU32::new(RECOVER);
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    /// Kind, address, pc, sp, frame size and data pointer of the last call: the handler may not panic, so the resumed thread asserts.
    static SEEN: [AtomicUsize; 6] = [const { AtomicUsize::new(0) }; 6];
    fn cookie() -> *mut c_void { &COOKIE as *const AtomicU32 as *mut c_void }

    unsafe extern "C" fn on_fault(kind: u32, address: usize, frame: *mut c_void, size: usize, data: *mut c_void) -> u32 {
        // A process the port ends cannot say why, so a child that is about to be ended says on stderr that its handler ran.
        if MODE.load(SeqCst) != RECOVER && unsafe { libc::write(2, CALLED.as_ptr().cast(), CALLED.len()) } != CALLED.len() as isize { unsafe { libc::_exit(3) }; }
        if MODE.load(SeqCst) == NEST { unsafe { pal_std_fault_load(32) }; }
        if MODE.load(SeqCst) != RECOVER || frame.is_null() || size != size_of::<Frame>() { return UNHANDLED; }
        let (pc, result, second, sp) = registers(unsafe { &mut *frame.cast::<Frame>() });
        for (slot, value) in SEEN.iter().zip([kind as usize, address, *pc as usize, sp as usize, size, data as usize]) { slot.store(value, SeqCst); }
        CALLS.fetch_add(1, SeqCst);
        let [load, loaded, store, stored] = sites();
        #[cfg(target_arch = "aarch64")]
        if *pc == skew_sites()[0] { (*result, *pc) = (MARKER, skew_sites()[1]); return RESUME; }
        if *pc == load {
            (*result, *second, *pc) = (MARKER | address as u64, SECOND, loaded);
        } else if *pc == store {
            *pc = stored;
        } else {
            return UNHANDLED;
        }
        RESUME
    }
    /// Takes one fault on the calling thread and checks what the handler saw of it.
    fn provoke(address: usize, keeps: bool) {
        let (calls, local) = (CALLS.load(SeqCst), 0u8);
        let [load, _, store, _] = sites();
        let (site, returned) = if keeps { (store, unsafe { pal_std_fault_keeps(address) }) } else { (load, unsafe { pal_std_fault_load(address) }) };
        assert_eq!(returned, if keeps { 0 } else { (MARKER | address as u64).wrapping_add(SECOND) }, "registers the handler {}", if keeps { "left alone" } else { "edited" });
        let [kind, at, pc, sp, size, data] = seen();
        assert_eq!(CALLS.load(SeqCst), calls + 1);
        assert_eq!((kind as u32, at, pc, size, data), (ACCESS, address, site as usize, size_of::<Frame>(), cookie() as usize));
        assert_eq!(COOKIE.load(SeqCst), 0x1234);
        // The interrupted sp is this thread's: just under this frame, inside the stack the port reports for the thread.
        let (mut low, mut high) = (ptr::null_mut(), ptr::null_mut());
        assert_eq!(unsafe { api().kernel.stack_bounds.unwrap()(&mut low, &mut high) }, OK);
        let here = std::hint::black_box(&local) as *const u8 as usize;
        assert!(sp <= here && here - sp < 65536 && sp >= low as usize && sp < high as usize, "sp {sp:#x} local {here:#x} stack {low:?}..{high:?}");
        // The handler returned through the kernel: the fault's signal is deliverable again.
        let mut mask: libc::sigset_t = unsafe { std::mem::zeroed() };
        assert_eq!(unsafe { libc::pthread_sigmask(libc::SIG_SETMASK, ptr::null(), &mut mask) }, 0);
        assert!(unsafe { libc::sigismember(&mask, libc::SIGSEGV) == 0 && libc::sigismember(&mask, libc::SIGBUS) == 0 });
    }
    /// Runs this test again in a process of its own that answers in `mode`, and returns the signal that ended it, if one did, and how
    /// often its handler ran then. Not a fork: on macOS the child side of a fork from a process with other threads was seen to die inside
    /// fork itself, in libSystem's atfork handler, on an os_once another thread was in the middle of.
    fn dies(mode: u32) -> (Option<i32>, usize) {
        use std::os::unix::process::ExitStatusExt;
        let output = std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact", NAME, "--test-threads=1"]).env(CHILD, mode.to_string()).output().unwrap();
        (output.status.signal(), output.stderr.windows(CALLED.len()).filter(|window| *window == CALLED).count())
    }
    /// The child of `dies`: one fault it resumes from, then the one that ends it. Surviving fails the parent's assertion.
    fn child(mode: u32) -> ! {
        unsafe { libc::setrlimit(libc::RLIMIT_CORE, &libc::rlimit { rlim_cur: 0, rlim_max: 0 }) };
        unsafe { libc::alarm(10) };
        provoke(16, false);
        MODE.store(mode, SeqCst);
        // Sent to this thread: a signal for the process may run its handler on another thread while this one goes on to exit.
        if mode == SENT { unsafe { libc::raise(libc::SIGBUS) }; } else { unsafe { pal_std_fault_load(16) }; }
        std::process::exit(0)
    }
    fn stats() -> faults::Stats {
        let mut out = faults::Stats::default();
        assert_eq!(unsafe { api().faults.read_stats.unwrap()(&mut out, size_of::<faults::Stats>()) }, OK);
        out
    }

    /// One test: install is once per process, and no other test of this binary faults.
    #[test]
    fn faults_are_reported_resumed_and_end_the_process_when_declined() {
        assert_eq!(api().header.capabilities & faults::CAP, faults::CAP);
        let f = &api().faults;
        assert_eq!(f.frame_tag.unwrap()(), if cfg!(target_arch = "aarch64") { faults::FRAME_ARM64 } else { faults::FRAME_X64 });
        assert_eq!(f.frame_size.unwrap()(), size_of::<Frame>());
        assert_eq!(unsafe { f.install.unwrap()(None, cookie()) }, INVALID_ARGUMENT);
        assert_eq!(unsafe { f.install.unwrap()(Some(on_fault), cookie()) }, OK);
        assert_eq!(unsafe { f.install.unwrap()(Some(on_fault), cookie()) }, BUSY);
        let armed = stats();
        assert_eq!((armed.installs, armed.delivered, armed.rejected), (1, 0, 2));
        if let Ok(mode) = std::env::var(CHILD) { child(mode.parse().unwrap()); }
        assert_eq!(sites()[1] - sites()[0], if cfg!(target_arch = "aarch64") { 4 } else { 3 }, "the length of the load");
        // Twice on this thread: a resumed fault leaves the port ready for the next one. 16 is a null reference plus a field offset.
        provoke(0, false);
        provoke(16, false);
        provoke(0, true);
        // A protected page is an invalid access like an unmapped one, whichever signal the kernel has for it.
        let (vm, mut page) = (&api().vm, ptr::null_mut());
        let size = unsafe { vm.page_size.unwrap()() };
        assert_eq!(unsafe { vm.reserve.unwrap()(size, size, 0, &mut page) }, OK);
        provoke(page as usize + 8, false);
        assert_eq!(unsafe { vm.release.unwrap()(page, size) }, OK);
        #[cfg(target_arch = "aarch64")]
        {
            assert_eq!(unsafe { pal_std_fault_skew() }, MARKER);
            let [kind, address, pc, sp, ..] = seen();
            assert_eq!((kind as u32, pc as u64, sp % 16), (faults::ALIGNMENT, skew_sites()[0], 8), "address {address:#x} sp {sp:#x}");
        }
        // The handler runs on the thread that faulted, on that thread's stack.
        std::thread::spawn(|| { provoke(16, false); provoke(24, true); }).join().unwrap();
        // UNHANDLED ends the process by the fault's own signal, and so does a fault inside the handler, which is not reported
        // a second time. A signal somebody sent is not a fault and reaches no handler, even on a thread that has taken a fault before.
        assert_eq!(dies(DECLINE), (Some(libc::SIGSEGV), 1));
        assert_eq!(dies(NEST), (Some(libc::SIGSEGV), 1));
        assert_eq!(dies(SENT), (Some(libc::SIGBUS), 0));
        // The children counted their own faults; this process resumed every one it took.
        let taken = CALLS.load(SeqCst) as u64;
        assert_eq!(unsafe { f.read_stats.unwrap()(ptr::null_mut(), size_of::<faults::Stats>()) }, INVALID_ARGUMENT);
        assert_eq!(unsafe { f.read_stats.unwrap()(&mut faults::Stats::default(), size_of::<faults::Stats>() - 1) }, INVALID_ARGUMENT);
        let after = stats();
        let expected = if cfg!(target_arch = "aarch64") { 7 } else { 6 };
        assert_eq!((taken, after.installs, after.delivered, after.resumed, after.unhandled, after.rejected), (expected, 1, expected, expected, 0, 2));
    }
}

#[cfg(not(all(any(target_os = "linux", target_os = "macos"), any(target_arch = "aarch64", target_arch = "x86_64"))))]
#[test]
fn faults_are_absent_without_a_signal_context() {
    let api = api();
    assert_eq!(api.header.capabilities & dotnet_pal_rs::faults::CAP, 0);
    assert!(api.faults.install.is_none() && api.faults.frame_tag.is_none() && api.faults.read_stats.is_some());
}

mod sockets_table {
    use super::api;
    use dotnet_pal_rs::io::{ADDRESS_IN_USE, ADDRESS_NOT_AVAILABLE, ALREADY_CONNECTED, BROKEN_PIPE, CONNECTION_REFUSED, CONNECTION_RESET, IN_PROGRESS, MESSAGE_TOO_LARGE, NOT_CONNECTED, WOULD_BLOCK};
    use dotnet_pal_rs::kernel::{INFINITE, TIMEOUT};
    use dotnet_pal_rs::runtime::{BUFFER_TOO_SMALL, NOT_FOUND};
    use dotnet_pal_rs::sockets::{self, Address, PollEntry, DATAGRAM as UDP, IPV4 as V4, IPV6 as V6, NO_CHANNEL, POLL_ERROR as ERROR, POLL_HANGUP as HANGUP,
        POLL_READ as READ, POLL_WRITE as WRITE, RECEIVE_PEEK, STREAM as TCP};
    use dotnet_pal_rs::{INVALID_ARGUMENT, OK, UNSUPPORTED};
    use std::{ffi::c_void, mem::size_of, ptr, sync::{atomic::{AtomicBool, Ordering}, Arc}, time::{Duration, Instant}};

    const MS: u64 = 1_000_000;
    /// The longest a step that has to succeed may take, in milliseconds: a bug ends in a failed assertion rather than a wait.
    const LIMIT: u64 = 5000;
    /// Ends the run when a test outlives its minute: a call that never returns must not hang `cargo test`.
    struct Watchdog(Arc<AtomicBool>);
    impl Drop for Watchdog { fn drop(&mut self) { self.0.store(true, Ordering::SeqCst); } }
    fn watchdog(test: &'static str) -> Watchdog {
        let done = Arc::new(AtomicBool::new(false));
        let seen = done.clone();
        std::thread::spawn(move || {
            for _ in 0..600 { std::thread::sleep(Duration::from_millis(100)); if seen.load(Ordering::SeqCst) { return; } }
            eprintln!("{test} did not finish within a minute");
            std::process::abort();
        });
        Watchdog(done)
    }

    fn ops() -> &'static sockets::Ops { &api().sockets }
    /// What an output holds before a call: a failed call has to leave NULL there instead.
    fn stale() -> *mut c_void { ptr::dangling_mut::<u64>().cast() }
    fn loopback(family: u32, port: u16) -> Address {
        if family == V4 { Address::v4([127, 0, 0, 1], port) } else { Address::v6([0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1], port, 0) }
    }
    fn create(family: u32, kind: u32) -> (u32, *mut c_void) {
        let mut socket = stale();
        (unsafe { ops().create.unwrap()(family, kind, &mut socket) }, socket)
    }
    fn close(socket: *mut c_void) -> u32 { unsafe { ops().close.unwrap()(socket) } }
    fn get_option(socket: *mut c_void, name: u32) -> (u32, u64) {
        let mut value = 7u64;
        (unsafe { ops().get_option.unwrap()(socket, name, &mut value) }, value)
    }
    fn option(socket: *mut c_void, name: u32) -> u64 { let (code, value) = get_option(socket, name); assert_eq!(code, OK, "option {name}"); value }
    fn set_option(socket: *mut c_void, name: u32, value: u64) -> u32 { unsafe { ops().set_option.unwrap()(socket, name, value) } }
    fn round_trip(socket: *mut c_void, name: u32, value: u64, low: u64, high: u64) {
        assert_eq!(set_option(socket, name, value), OK, "option {name}");
        let got = option(socket, name);
        assert!((low..=high).contains(&got), "option {name}: {value} read back as {got}");
    }
    /// Blocking transfers on `socket` give up after `LIMIT`.
    fn bounded(socket: *mut c_void) -> *mut c_void {
        assert_eq!((set_option(socket, sockets::RECEIVE_TIMEOUT, LIMIT), set_option(socket, sockets::SEND_TIMEOUT, LIMIT)), (OK, OK));
        socket
    }
    fn open(family: u32, kind: u32) -> *mut c_void {
        let (code, socket) = create(family, kind);
        assert_eq!(code, OK);
        assert!(!socket.is_null());
        bounded(socket)
    }
    fn bind(socket: *mut c_void, address: &Address) -> u32 { unsafe { ops().bind.unwrap()(socket, address) } }
    fn connect(socket: *mut c_void, address: &Address) -> u32 { unsafe { ops().connect.unwrap()(socket, address) } }
    fn shutdown(socket: *mut c_void, how: u32) -> u32 { unsafe { ops().shutdown.unwrap()(socket, how) } }
    fn set_blocking(socket: *mut c_void, blocking: u32) -> u32 { unsafe { ops().set_blocking.unwrap()(socket, blocking) } }
    fn local(socket: *mut c_void) -> Address {
        let mut out = Address { family: 9, ..Address::default() };
        assert_eq!(unsafe { ops().local_address.unwrap()(socket, &mut out) }, OK);
        out
    }
    fn peer(socket: *mut c_void) -> (u32, Address) {
        let mut out = Address { family: 9, ..Address::default() };
        (unsafe { ops().peer_address.unwrap()(socket, &mut out) }, out)
    }
    /// A socket bound to a loopback port the OS picked.
    fn bound(family: u32, kind: u32) -> (*mut c_void, Address) {
        let socket = open(family, kind);
        assert_eq!(bind(socket, &loopback(family, 0)), OK);
        let at = local(socket);
        assert!(at.port != 0);
        assert_eq!(at, loopback(family, at.port));
        (socket, at)
    }
    fn listener(family: u32) -> (*mut c_void, Address) {
        let (socket, at) = bound(family, TCP);
        assert_eq!(unsafe { ops().listen.unwrap()(socket, 8) }, OK);
        (socket, at)
    }
    fn accept(server: *mut c_void) -> (u32, *mut c_void, Address) {
        let (mut accepted, mut from) = (stale(), Address { family: 9, ..Address::default() });
        (unsafe { ops().accept.unwrap()(server, &mut accepted, &mut from) }, accepted, from)
    }
    fn poll(entries: &mut [PollEntry], timeout_ns: u64, channel: u32) -> (u32, usize) {
        let mut ready = 7usize;
        (unsafe { ops().poll.unwrap()(if entries.is_empty() { ptr::null_mut() } else { entries.as_mut_ptr() }, entries.len(), timeout_ns, channel, &mut ready) }, ready)
    }
    fn bits(socket: *mut c_void, requested: u32, timeout_ns: u64) -> u32 {
        let mut entry = [PollEntry { socket, requested, triggered: 99 }];
        assert_eq!(poll(&mut entry, timeout_ns, NO_CHANNEL), (OK, (entry[0].triggered != 0) as usize));
        entry[0].triggered
    }
    /// A blocking connection through `server`. The accept waits on poll, which has a limit on every OS; the peer the server sees is the client's own address.
    fn connected(server: *mut c_void, at: &Address) -> (*mut c_void, *mut c_void) {
        let client = open(at.family as u32, TCP);
        assert_eq!(connect(client, at), OK);
        assert_eq!(bits(server, READ, LIMIT * MS), READ);
        let (code, accepted, from) = accept(server);
        assert_eq!(code, OK);
        assert!(!accepted.is_null());
        assert_eq!((local(client), peer(accepted)), (from, (OK, from)));
        assert_eq!((peer(client), local(accepted)), ((OK, *at), *at));
        (client, bounded(accepted))
    }
    fn send(socket: *mut c_void, data: &[u8], to: Option<&Address>) -> (u32, usize) {
        let mut done = 7usize;
        (unsafe { ops().send.unwrap()(socket, if data.is_empty() { ptr::null() } else { data.as_ptr() }, data.len(), to.map_or(ptr::null(), |to| to), &mut done) }, done)
    }
    fn receive(socket: *mut c_void, buffer: &mut [u8], flags: u32) -> (u32, usize, Address) {
        let (mut done, mut from) = (7usize, Address { family: 9, ..Address::default() });
        (unsafe { ops().receive.unwrap()(socket, buffer.as_mut_ptr(), buffer.len(), flags, &mut from, &mut done) }, done, from)
    }
    /// Sends `text` one way and expects exactly it on the blocking other side.
    fn transfer(from: *mut c_void, to: *mut c_void, text: &[u8]) {
        assert_eq!(send(from, text, None), (OK, text.len()));
        let (mut buffer, mut total) = ([0u8; 64], 0);
        while total < text.len() {
            let (code, done, _) = receive(to, &mut buffer[total..], 0);
            assert!(code == OK && done > 0, "status {code} after {total} bytes");
            total += done;
        }
        assert_eq!(&buffer[..total], text);
    }
    fn wake(channel: u32) -> u32 { unsafe { ops().wake.unwrap()(channel) } }
    fn stats() -> sockets::Stats {
        let mut out = sockets::Stats::default();
        assert_eq!(unsafe { ops().read_stats.unwrap()(&mut out, size_of::<sockets::Stats>()) }, OK);
        out
    }
    fn resolve(name: &[u8], family: u32, found: &mut [Address]) -> (u32, usize) {
        let mut count = 7usize;
        (unsafe { ops().resolve.unwrap()(name.as_ptr(), name.len(), family, found.as_mut_ptr(), found.len(), &mut count) }, count)
    }

    #[test]
    fn sockets_stream_transfers_peeks_and_half_closes() {
        let _watchdog = watchdog("sockets_stream_transfers_peeks_and_half_closes");
        assert_eq!(api().header.capabilities & sockets::CAP, sockets::CAP);
        let s = ops();
        assert!(s.create.is_some() && s.close.is_some() && s.bind.is_some() && s.listen.is_some() && s.accept.is_some() && s.connect.is_some() && s.send.is_some()
            && s.receive.is_some() && s.shutdown.is_some() && s.local_address.is_some() && s.peer_address.is_some() && s.set_blocking.is_some() && s.get_option.is_some()
            && s.set_option.is_some() && s.poll.is_some() && s.wake.is_some() && s.resolve.is_some() && s.host_name.is_some() && s.read_stats.is_some());
        let before = stats();
        let (server, at) = listener(V4);
        let (client, accepted) = connected(server, &at);
        // Both directions; a stream names no sender, so `from` stays the zero address.
        let mut buffer = [0u8; 64];
        assert_eq!(send(client, b"ping", None), (OK, 4));
        assert_eq!(receive(accepted, &mut buffer, 0), (OK, 4, Address::default()));
        assert_eq!(&buffer[..4], b"ping");
        transfer(accepted, client, b"pong!");
        // Peeking leaves the bytes for the next receive, and AVAILABLE counts them.
        assert_eq!(send(accepted, b"again", None), (OK, 5));
        assert_eq!(bits(client, READ, LIMIT * MS), READ);
        assert_eq!(receive(client, &mut buffer, RECEIVE_PEEK), (OK, 5, Address::default()));
        assert_eq!((&buffer[..5], option(client, sockets::AVAILABLE)), (&b"again"[..], 5));
        buffer.fill(0);
        assert_eq!(receive(client, &mut buffer, 0), (OK, 5, Address::default()));
        assert_eq!((&buffer[..5], option(client, sockets::AVAILABLE)), (&b"again"[..], 0));
        assert_eq!(send(client, &[], None), (OK, 0), "an empty stream send has nothing to do");
        // Half close: the peer reads end of stream, the other direction keeps working and the closed one is a broken pipe.
        assert_eq!(shutdown(client, sockets::SHUTDOWN_WRITE), OK);
        assert_eq!(receive(accepted, &mut buffer, 0), (OK, 0, Address::default()));
        assert_eq!((bits(accepted, READ, 0), bits(accepted, WRITE, 0)), (READ | HANGUP, WRITE));
        assert_eq!(bits(accepted, READ | WRITE, 0), READ | WRITE | HANGUP, "a half-closed connection is still writable");
        // One direction down is no hangup for an entry that does not read, and it does not cut such an entry's wait short either.
        let start = Instant::now();
        assert_eq!(bits(accepted, 0, 100 * MS), 0);
        assert!(start.elapsed() >= Duration::from_millis(95));
        transfer(accepted, client, b"late");
        assert_eq!(send(client, b"x", None), (BROKEN_PIPE, 0));
        assert_eq!(connect(client, &at), ALREADY_CONNECTED);
        // Both directions down: HANGUP whether or not anything was requested.
        assert_eq!(shutdown(accepted, sockets::SHUTDOWN_WRITE), OK);
        assert_eq!(bits(accepted, 0, LIMIT * MS), HANGUP);
        assert_eq!(bits(client, 0, LIMIT * MS), HANGUP);
        assert_eq!(bits(client, READ, 0) & (READ | HANGUP), READ | HANGUP);
        assert_eq!((close(client), close(accepted)), (OK, OK));
        // A socket with no connection, and the listener's port being taken.
        let idle = open(V4, TCP);
        assert_eq!(peer(idle), (NOT_CONNECTED, Address::default()));
        assert_eq!(receive(idle, &mut buffer, 0), (NOT_CONNECTED, 0, Address::default()));
        assert_eq!(bind(idle, &at), ADDRESS_IN_USE);
        assert_eq!((close(idle), close(server)), (OK, OK));
        // Nobody listens on a port that was just closed. (A port that is bound and not listening would not do: macOS leaves such a connect unanswered.)
        let refused = open(V4, TCP);
        assert_eq!(connect(refused, &at), CONNECTION_REFUSED);
        assert_eq!(close(refused), OK);
        let after = stats();
        for (name, was, now) in [("create", before.create_ok, after.create_ok), ("close", before.close_ok, after.close_ok), ("bind", before.bind_ok, after.bind_ok),
            ("listen", before.listen_ok, after.listen_ok), ("accept", before.accept_ok, after.accept_ok), ("connect", before.connect_ok, after.connect_ok),
            ("send", before.send_ok, after.send_ok), ("receive", before.receive_ok, after.receive_ok), ("shutdown", before.shutdown_ok, after.shutdown_ok),
            ("address", before.address_ok, after.address_ok), ("option", before.option_ok, after.option_ok), ("poll", before.poll_ok, after.poll_ok),
            ("failed", before.rejected_or_failed, after.rejected_or_failed)] {
            assert!(now > was, "{name} counter did not move: {was} -> {now}");
        }
    }

    #[test]
    fn sockets_non_blocking_calls_and_expired_timeouts() {
        let _watchdog = watchdog("sockets_non_blocking_calls_and_expired_timeouts");
        let (server, at) = listener(V4);
        let client = open(V4, TCP);
        let mut buffer = [0u8; 8];
        assert_eq!((set_blocking(server, 0), set_blocking(client, 0)), (OK, OK));
        assert_eq!(accept(server), (WOULD_BLOCK, ptr::null_mut(), Address::default()));
        let status = connect(client, &at);
        assert!(status == IN_PROGRESS || status == OK, "status {status}");
        // Completion shows as writability with no pending error.
        assert_eq!((bits(client, WRITE, LIMIT * MS), option(client, sockets::ERROR)), (WRITE, OK as u64));
        assert_eq!(receive(client, &mut buffer, 0), (WOULD_BLOCK, 0, Address::default()));
        assert_eq!(bits(server, READ, LIMIT * MS), READ);
        let (code, accepted, from) = accept(server);
        assert_eq!((code, from), (OK, local(client)));
        // The accepted socket starts blocking whatever the listener is: with a receive timeout it waits, then reports the expiry.
        assert_eq!(set_option(accepted, sockets::RECEIVE_TIMEOUT, 200), OK);
        let start = Instant::now();
        assert_eq!(receive(accepted, &mut buffer, 0), (TIMEOUT, 0, Address::default()));
        let waited = start.elapsed();
        assert!(waited >= Duration::from_millis(150) && waited < Duration::from_millis(LIMIT), "waited {waited:?}");
        assert_eq!((set_option(accepted, sockets::RECEIVE_TIMEOUT, LIMIT), set_blocking(client, 1)), (OK, OK));
        transfer(accepted, client, b"blocking again");
        // A send nobody reads fills both buffers; on a blocking socket the expired SEND_TIMEOUT is TIMEOUT as well.
        assert_eq!((set_option(accepted, sockets::SEND_BUFFER, 16384), set_option(accepted, sockets::SEND_TIMEOUT, 200)), (OK, OK));
        let (chunk, mut status, mut total) = (vec![0x5au8; 65536], OK, 0usize);
        for _ in 0..4096 {
            let (code, done) = send(accepted, &chunk, None);
            if code != OK { assert_eq!(done, 0); status = code; break; }
            total += done;
        }
        assert_eq!(status, TIMEOUT, "after {total} bytes");
        assert_eq!((close(client), close(accepted), close(server)), (OK, OK, OK));
        // A failed non-blocking connect reports through poll, requested or not, and through the ERROR option, which clears on read.
        let (gone, closed) = listener(V4);
        let refused = open(V4, TCP);
        assert_eq!((close(gone), set_blocking(refused, 0)), (OK, OK));
        let status = connect(refused, &closed);
        assert!(status == IN_PROGRESS || status == CONNECTION_REFUSED, "status {status}");
        if status == IN_PROGRESS {
            assert_eq!(bits(refused, 0, LIMIT * MS), ERROR | HANGUP);
            assert_eq!(bits(refused, WRITE, 0), WRITE | ERROR | HANGUP);
            assert_eq!((option(refused, sockets::ERROR), option(refused, sockets::ERROR)), (CONNECTION_REFUSED as u64, OK as u64));
        }
        assert_eq!(close(refused), OK);
    }

    #[test]
    fn sockets_poll_is_level_triggered_and_wakes_per_channel() {
        let _watchdog = watchdog("sockets_poll_is_level_triggered_and_wakes_per_channel");
        let before = stats().wake_ok;
        let (server, at) = listener(V4);
        let (client, accepted) = connected(server, &at);
        let ms = Duration::from_millis;
        // Expiry: nothing is readable, so the whole time passes and nothing is triggered, with or without a channel.
        let mut entry = [PollEntry { socket: client, requested: READ, triggered: 99 }];
        let start = Instant::now();
        assert_eq!((poll(&mut entry, 100 * MS, NO_CHANNEL), entry[0].triggered), ((OK, 0), 0));
        assert!(start.elapsed() >= ms(95) && start.elapsed() < ms(LIMIT));
        let start = Instant::now();
        assert_eq!(poll(&mut entry, 50 * MS, 0), (OK, 0));
        assert!(start.elapsed() >= ms(45));
        // An empty poll is a plain wait for its channel's wake; a fraction of the OS's unit still waits.
        let start = Instant::now();
        assert_eq!(poll(&mut [], 50 * MS, sockets::POLL_CHANNELS - 1), (OK, 0));
        assert!(start.elapsed() >= ms(45));
        let start = Instant::now();
        assert_eq!(poll(&mut [], 1500, NO_CHANNEL), (OK, 0));
        assert!(start.elapsed() >= Duration::from_nanos(1500));
        // Level-triggered answers: only requested bits that are ready, at once with a zero timeout.
        assert_eq!((bits(client, READ, 0), bits(client, WRITE, 0), bits(client, 0, 0)), (0, WRITE, 0));
        assert_eq!(send(accepted, b"x", None), (OK, 1));
        assert_eq!((bits(client, READ, INFINITE), bits(client, READ, 0), bits(client, READ | WRITE, 0)), (READ, READ, READ | WRITE));
        // Many entries over two sockets, each listed 150 times; every other one has nothing to read.
        let mut many: Vec<PollEntry> = (0..300).map(|i| PollEntry { socket: if i % 2 == 1 { client } else { accepted }, requested: READ, triggered: 99 }).collect();
        assert_eq!(poll(&mut many, 0, 1), (OK, 150));
        for (i, entry) in many.iter().enumerate() { assert_eq!(entry.triggered, if i % 2 == 1 { READ } else { 0 }, "entry {i}"); }
        // One socket listed with different requests answers each entry for what it asked.
        let mut mixed = [READ, WRITE, 0, READ | WRITE].map(|requested| PollEntry { socket: client, requested, triggered: 99 });
        assert_eq!(poll(&mut mixed, 0, NO_CHANNEL), (OK, 3));
        assert_eq!(mixed.map(|entry| entry.triggered), [READ, WRITE, 0, READ | WRITE]);
        let mut byte = [0u8; 1];
        assert_eq!((receive(client, &mut byte, 0), byte), ((OK, 1, Address::default()), *b"x"));
        // wake from another thread ends the endless poll on its channel with nothing triggered.
        let waker = std::thread::spawn(|| { std::thread::sleep(Duration::from_millis(100)); wake(3) });
        let start = Instant::now();
        entry[0].triggered = 99;
        assert_eq!((poll(&mut entry, INFINITE, 3), entry[0].triggered), ((OK, 0), 0));
        assert!(start.elapsed() >= ms(50));
        assert_eq!(waker.join().unwrap(), OK);
        // A wake with no poll in progress waits for its own channel: polls on another channel or on none run to their timeout.
        assert_eq!(wake(4), OK);
        let start = Instant::now();
        assert_eq!(poll(&mut entry, 100 * MS, 5), (OK, 0));
        assert!(start.elapsed() >= ms(95));
        let start = Instant::now();
        assert_eq!(poll(&mut entry, 50 * MS, NO_CHANNEL), (OK, 0));
        assert!(start.elapsed() >= ms(45));
        // It ends the next poll on channel 4, and only that one.
        assert_eq!((poll(&mut entry, INFINITE, 4), entry[0].triggered), ((OK, 0), 0));
        let start = Instant::now();
        assert_eq!(poll(&mut entry, 50 * MS, 4), (OK, 0));
        assert!(start.elapsed() >= ms(45));
        // Several wakes before a poll are still one early return; a wake does not hide what is ready.
        assert_eq!((wake(6), wake(6), wake(6)), (OK, OK, OK));
        entry[0].requested = WRITE;
        assert_eq!((poll(&mut entry, INFINITE, 6), entry[0].triggered), ((OK, 1), WRITE));
        entry[0].requested = READ;
        let start = Instant::now();
        assert_eq!(poll(&mut entry, 50 * MS, 6), (OK, 0));
        assert!(start.elapsed() >= ms(45));
        // Channels end at POLL_CHANNELS; only a poll may go without one.
        assert_eq!(poll(&mut entry, 0, sockets::POLL_CHANNELS), (INVALID_ARGUMENT, 0));
        assert_eq!((wake(sockets::POLL_CHANNELS), wake(NO_CHANNEL)), (INVALID_ARGUMENT, INVALID_ARGUMENT));
        assert_eq!((close(client), close(accepted), close(server)), (OK, OK, OK));
        assert!(stats().wake_ok >= before + 5);
    }

    #[test]
    fn sockets_options_round_trip_in_the_boundary_encoding() {
        let _watchdog = watchdog("sockets_options_round_trip_in_the_boundary_encoding");
        let ((code, stream), (other, datagram)) = (create(V4, TCP), create(V4, UDP));
        assert_eq!((code, other), (OK, OK));
        for flag in [sockets::REUSE_ADDRESS, sockets::NO_DELAY, sockets::KEEP_ALIVE] {
            assert_eq!(option(stream, flag), 0, "option {flag}");
            round_trip(stream, flag, 1, 1, 1);
            round_trip(stream, flag, 0, 0, 0);
        }
        round_trip(datagram, sockets::BROADCAST, 1, 1, 1);
        round_trip(datagram, sockets::BROADCAST, 0, 0, 0);
        // Sizes are the OS's to round; they only have to stay real.
        assert!(option(stream, sockets::RECEIVE_BUFFER) != 0 && option(stream, sockets::SEND_BUFFER) != 0);
        round_trip(stream, sockets::RECEIVE_BUFFER, 65536, 1, u32::MAX as u64);
        round_trip(stream, sockets::SEND_BUFFER, 65536, 1, u32::MAX as u64);
        // LINGER: 0 is off, n + 1 is on with n seconds, so 1 is "on, drop at once".
        assert_eq!(option(stream, sockets::LINGER), 0);
        for value in [1, 6, 0] { round_trip(stream, sockets::LINGER, value, value, value); }
        // Timeouts are milliseconds; the OS may round up to its tick.
        assert_eq!((option(stream, sockets::RECEIVE_TIMEOUT), option(stream, sockets::SEND_TIMEOUT)), (0, 0));
        round_trip(stream, sockets::RECEIVE_TIMEOUT, 1500, 1500, 1520);
        round_trip(stream, sockets::SEND_TIMEOUT, 2500, 2500, 2520);
        round_trip(stream, sockets::RECEIVE_TIMEOUT, 0, 0, 0);
        round_trip(stream, sockets::SEND_TIMEOUT, 0, 0, 0);
        assert_eq!((option(stream, sockets::ERROR), option(datagram, sockets::AVAILABLE), option(stream, sockets::AVAILABLE)), (OK as u64, 0, 0));
        // An option the protocol does not have: no delay on datagrams, IPv6-only on IPv4.
        assert_eq!((get_option(datagram, sockets::NO_DELAY), set_option(datagram, sockets::NO_DELAY, 1)), ((UNSUPPORTED, 0), UNSUPPORTED));
        assert_eq!((get_option(stream, sockets::IPV6_ONLY), set_option(stream, sockets::IPV6_ONLY, 1)), ((UNSUPPORTED, 0), UNSUPPORTED));
        // ERROR and AVAILABLE are read-only, flags are 0 or 1, and no value needs more than 32 bits.
        assert_eq!((set_option(stream, sockets::ERROR, 0), set_option(stream, sockets::AVAILABLE, 0), set_option(stream, sockets::NO_DELAY, 2)), (INVALID_ARGUMENT, INVALID_ARGUMENT, INVALID_ARGUMENT));
        assert_eq!((set_option(stream, sockets::LINGER, 1 << 32), get_option(stream, 20).0, get_option(stream, 0).0), (INVALID_ARGUMENT, INVALID_ARGUMENT, INVALID_ARGUMENT));
        assert_eq!((close(stream), close(datagram)), (OK, OK));
    }

    /// What the OS itself holds and delivers, asked by the test. Tests run in parallel, so the descriptor behind a handle is found
    /// by what only that socket has: its family, its type and the port it is bound to.
    #[cfg(unix)]
    mod kernel {
        use std::{mem::{size_of, size_of_val, zeroed}, net::Ipv4Addr, ptr};
        pub fn option(fd: i32, level: i32, name: i32) -> Option<i32> {
            let (mut value, mut length) = (-7i32, size_of::<i32>() as libc::socklen_t);
            (unsafe { libc::getsockopt(fd, level, name, ptr::addr_of_mut!(value).cast(), &mut length) } == 0).then_some(value)
        }
        pub fn descriptor(v6: bool, stream: bool, port: u16) -> i32 {
            let (family, kind) = (if v6 { libc::AF_INET6 } else { libc::AF_INET }, if stream { libc::SOCK_STREAM } else { libc::SOCK_DGRAM });
            (0..4096).find(|&fd| {
                let (mut storage, mut length) = (unsafe { zeroed::<libc::sockaddr_storage>() }, size_of::<libc::sockaddr_storage>() as libc::socklen_t);
                if unsafe { libc::getsockname(fd, ptr::addr_of_mut!(storage).cast(), &mut length) } != 0 || storage.ss_family as i32 != family { return false; }
                // sin_port and sin6_port are the same two bytes.
                u16::from_be(unsafe { ptr::addr_of!(storage).cast::<libc::sockaddr_in>().read() }.sin_port) == port && option(fd, libc::SOL_SOCKET, libc::SO_TYPE) == Some(kind)
            }).expect("no descriptor is bound to the socket's port")
        }
        /// Makes `fd` report the hop limit and the arrival interface of what it receives.
        pub fn report(fd: i32) {
            let on = 1i32;
            for name in [libc::IP_RECVTTL, libc::IP_PKTINFO] {
                assert_eq!(unsafe { libc::setsockopt(fd, libc::IPPROTO_IP, name, ptr::addr_of!(on).cast(), size_of::<i32>() as libc::socklen_t) }, 0);
            }
        }
        /// `(hop limit, arrival interface)` of one datagram that reaches `fd` within `ms`.
        pub fn arrived(fd: i32, ms: i32) -> Option<(i32, u32)> {
            let mut wait = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
            if unsafe { libc::poll(&mut wait, 1, ms) } <= 0 { return None; }
            let (mut data, mut control) = ([0u8; 64], [0u64; 32]);
            let mut part = libc::iovec { iov_base: data.as_mut_ptr().cast(), iov_len: data.len() };
            let mut message: libc::msghdr = unsafe { zeroed() };
            (message.msg_iov, message.msg_iovlen, message.msg_control, message.msg_controllen) = (&mut part, 1, control.as_mut_ptr().cast(), size_of_val(&control) as _);
            assert!(unsafe { libc::recvmsg(fd, &mut message, 0) } >= 0);
            let (mut hops, mut interface, mut header) = (-1, 0, unsafe { libc::CMSG_FIRSTHDR(&message) });
            while !header.is_null() {
                let (level, kind, data) = unsafe { ((*header).cmsg_level, (*header).cmsg_type, libc::CMSG_DATA(header)) };
                // The index is an int on Linux and unsigned on macOS.
                #[allow(clippy::unnecessary_cast)]
                if level == libc::IPPROTO_IP && kind == libc::IP_PKTINFO { interface = unsafe { data.cast::<libc::in_pktinfo>().read_unaligned() }.ipi_ifindex as u32; }
                // Linux delivers the hop limit as an int named IP_TTL, macOS as one byte named IP_RECVTTL.
                #[cfg(not(target_vendor = "apple"))]
                if level == libc::IPPROTO_IP && kind == libc::IP_TTL { hops = unsafe { data.cast::<i32>().read_unaligned() }; }
                #[cfg(target_vendor = "apple")]
                if level == libc::IPPROTO_IP && kind == libc::IP_RECVTTL { hops = unsafe { data.read() } as i32; }
                header = unsafe { libc::CMSG_NXTHDR(&message, header) };
            }
            Some((hops, interface))
        }
        /// The first interface that is up, carries multicast and has an IPv4 address, and that address.
        pub fn multicast_interface() -> Option<(u32, Ipv4Addr)> {
            let mut list = ptr::null_mut();
            assert_eq!(unsafe { libc::getifaddrs(&mut list) }, 0);
            let (mut next, mut found) = (list, None);
            while let Some(entry) = unsafe { next.as_ref() } {
                next = entry.ifa_next;
                let wanted = (libc::IFF_UP | libc::IFF_RUNNING | libc::IFF_MULTICAST) as u32;
                if found.is_some() || entry.ifa_addr.is_null() || entry.ifa_flags & wanted != wanted || entry.ifa_flags & libc::IFF_LOOPBACK as u32 != 0 { continue; }
                if unsafe { (*entry.ifa_addr).sa_family } as i32 != libc::AF_INET { continue; }
                let address = unsafe { entry.ifa_addr.cast::<libc::sockaddr_in>().read_unaligned() }.sin_addr.s_addr;
                found = Some((unsafe { libc::if_nametoindex(entry.ifa_name) }, Ipv4Addr::from(address.to_ne_bytes())));
            }
            unsafe { libc::freeifaddrs(list) };
            found.filter(|(index, _)| *index != 0)
        }
        pub fn loopback_index() -> u32 { [c"lo", c"lo0"].iter().map(|name| unsafe { libc::if_nametoindex(name.as_ptr()) }).find(|index| *index != 0).expect("no loopback interface") }
        /// The IPv6 hop limit the OS applies to a socket nobody set one on.
        pub fn default_hops_v6() -> i32 {
            #[cfg(target_vendor = "apple")]
            {
                let (mut limit, mut size) = (0i32, size_of::<i32>());
                assert_eq!(unsafe { libc::sysctlbyname(c"net.inet6.ip6.hlim".as_ptr(), ptr::addr_of_mut!(limit).cast(), &mut size, ptr::null_mut(), 0) }, 0);
                limit
            }
            #[cfg(not(target_vendor = "apple"))]
            { std::fs::read_to_string("/proc/sys/net/ipv6/conf/default/hop_limit").ok().and_then(|text| text.trim().parse().ok()).unwrap_or(64) }
        }
        #[cfg(target_vendor = "apple")]
        pub const KEEP_ALIVE_IDLE: i32 = libc::TCP_KEEPALIVE;
        #[cfg(not(target_vendor = "apple"))]
        pub const KEEP_ALIVE_IDLE: i32 = libc::TCP_KEEPIDLE;
    }
    fn unsupported(socket: *mut c_void, options: [u32; 3]) {
        for name in options { assert_eq!((get_option(socket, name), set_option(socket, name, 1)), ((UNSUPPORTED, 0), UNSUPPORTED), "option {name}"); }
    }
    const KEEP_ALIVE_TUNING: [u32; 3] = [sockets::KEEP_ALIVE_IDLE, sockets::KEEP_ALIVE_INTERVAL, sockets::KEEP_ALIVE_COUNT];
    const MULTICAST: [u32; 3] = [sockets::MULTICAST_HOPS, sockets::MULTICAST_LOOPBACK, sockets::MULTICAST_INTERFACE];

    #[test]
    fn sockets_tune_keep_alive_hop_limits_and_multicast() {
        let _watchdog = watchdog("sockets_tune_keep_alive_hop_limits_and_multicast");
        // The datagram socket takes every local address: macOS sends nothing to a group from a socket bound to loopback (ADDRESS_NOT_AVAILABLE).
        let ((stream, stream_at), datagram) = (bound(V4, TCP), open(V4, UDP));
        assert_eq!(bind(datagram, &Address::v4([0; 4], 0)), OK);
        let datagram_at = local(datagram);
        // Keep-alive tuning starts at the OS's defaults, round-trips in seconds and probes, and does not switch keep-alive on.
        let defaults = KEEP_ALIVE_TUNING.map(|name| option(stream, name));
        assert!(defaults.iter().all(|value| (1..=u32::MAX as u64).contains(value)), "{defaults:?}");
        for (name, value) in [(sockets::KEEP_ALIVE_IDLE, 123), (sockets::KEEP_ALIVE_INTERVAL, 45), (sockets::KEEP_ALIVE_COUNT, 6)] { round_trip(stream, name, value, value, value); }
        assert_eq!(option(stream, sockets::KEEP_ALIVE), 0);
        // No time and no count is refused and changes nothing.
        for name in KEEP_ALIVE_TUNING { assert_eq!(set_option(stream, name, 0), INVALID_ARGUMENT, "option {name}"); }
        assert_eq!(KEEP_ALIVE_TUNING.map(|name| option(stream, name)), [123, 45, 6]);
        // An option the protocol does not have: keep-alive tuning on datagrams, multicast on streams.
        unsupported(datagram, KEEP_ALIVE_TUNING);
        unsupported(stream, MULTICAST);
        // Hop limits on both kinds of socket; multicast starts at one hop, looped back, on the OS's choice of interface.
        let hops = (option(stream, sockets::HOPS), option(datagram, sockets::HOPS));
        assert!((1..=255).contains(&hops.0) && hops.0 == hops.1, "{hops:?}");
        round_trip(stream, sockets::HOPS, 33, 33, 33);
        round_trip(datagram, sockets::HOPS, 255, 255, 255);
        round_trip(datagram, sockets::HOPS, 44, 44, 44);
        assert_eq!(MULTICAST.map(|name| option(datagram, name)), [1, 1, 0]);
        for value in [0, 255, 5] { round_trip(datagram, sockets::MULTICAST_HOPS, value, value, value); }
        round_trip(datagram, sockets::MULTICAST_LOOPBACK, 0, 0, 0);
        // A hop limit is one byte and MULTICAST_LOOPBACK a flag; an interface that does not exist is refused; none of that changes a setting.
        assert_eq!((set_option(datagram, sockets::HOPS, 256), set_option(datagram, sockets::MULTICAST_HOPS, 256), set_option(datagram, sockets::MULTICAST_LOOPBACK, 2)),
            (INVALID_ARGUMENT, INVALID_ARGUMENT, INVALID_ARGUMENT));
        assert_eq!((set_option(datagram, sockets::MULTICAST_INTERFACE, 999_999), set_option(datagram, sockets::MULTICAST_INTERFACE, 1 << 32)), (ADDRESS_NOT_AVAILABLE, INVALID_ARGUMENT));
        assert_eq!((set_option(datagram, 20, 1), get_option(datagram, 20)), (INVALID_ARGUMENT, (INVALID_ARGUMENT, 0)));
        assert_eq!((option(datagram, sockets::HOPS), MULTICAST.map(|name| option(datagram, name))), (44, [5, 0, 0]));
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            // The OS holds the same values for the descriptors behind the handles.
            let (fd, datagram_fd) = (kernel::descriptor(false, true, stream_at.port), kernel::descriptor(false, false, datagram_at.port));
            let held = |fd, level, name| kernel::option(fd, level, name).expect("getsockopt");
            assert_eq!([kernel::KEEP_ALIVE_IDLE, libc::TCP_KEEPINTVL, libc::TCP_KEEPCNT].map(|name| held(fd, libc::IPPROTO_TCP, name)), [123, 45, 6]);
            assert_eq!((held(fd, libc::SOL_SOCKET, libc::SO_KEEPALIVE), held(fd, libc::IPPROTO_IP, libc::IP_TTL), held(datagram_fd, libc::IPPROTO_IP, libc::IP_TTL)), (0, 33, 44));
            assert_eq!((held(datagram_fd, libc::IPPROTO_IP, libc::IP_MULTICAST_TTL), held(datagram_fd, libc::IPPROTO_IP, libc::IP_MULTICAST_LOOP)), (5, 0));
            // Behaviour, seen by a socket of the test's own: a unicast datagram carries HOPS.
            let receiver = std::net::UdpSocket::bind("0.0.0.0:0").unwrap();
            kernel::report(receiver.as_raw_fd());
            let port = receiver.local_addr().unwrap().port();
            assert_eq!(send(datagram, b"hops", Some(&loopback(V4, port))), (OK, 4));
            assert_eq!(kernel::arrived(receiver.as_raw_fd(), LIMIT as i32).map(|(hops, _)| hops), Some(44));
            // A multicast datagram leaves through MULTICAST_INTERFACE with MULTICAST_HOPS, and comes back only while MULTICAST_LOOPBACK is on.
            match kernel::multicast_interface() {
                Some((index, address)) => {
                    let group = std::net::Ipv4Addr::new(239, 255, 77, 79);
                    receiver.join_multicast_v4(&group, &address).unwrap();
                    let to = Address::v4(group.octets(), port);
                    round_trip(datagram, sockets::MULTICAST_INTERFACE, index as u64, index as u64, index as u64);
                    assert_eq!((send(datagram, b"quiet", Some(&to)), kernel::arrived(receiver.as_raw_fd(), 300)), ((OK, 5), None));
                    round_trip(datagram, sockets::MULTICAST_LOOPBACK, 1, 1, 1);
                    assert_eq!((send(datagram, b"group", Some(&to)), kernel::arrived(receiver.as_raw_fd(), LIMIT as i32)), ((OK, 5), Some((5, index))));
                    // Through loopback instead, the same datagram does not reach a member on that interface: the option steers the traffic.
                    let aside = kernel::loopback_index() as u64;
                    round_trip(datagram, sockets::MULTICAST_INTERFACE, aside, aside, aside);
                    let _ = send(datagram, b"aside", Some(&to));
                    assert_eq!(kernel::arrived(receiver.as_raw_fd(), 300), None);
                    round_trip(datagram, sockets::MULTICAST_INTERFACE, 0, 0, 0);
                    println!("SOCKETS note: multicast traffic checked on interface {index} ({address})");
                }
                None => println!("SOCKETS note: no interface that is up, carries multicast and has an IPv4 address; the multicast traffic checks are skipped"),
            }
        }
        #[cfg(not(unix))]
        let _ = (stream_at, datagram_at);
        assert_eq!((close(stream), close(datagram)), (OK, OK));

        // The same options at the IPv6 level, where the host has IPv6.
        let (mut status, probe) = create(V6, UDP);
        if status == OK { status = bind(probe, &loopback(V6, 0)); assert_eq!(close(probe), OK); }
        if status == ADDRESS_NOT_AVAILABLE || status == UNSUPPORTED { println!("SOCKETS note: no IPv6 loopback here (status {status}), the IPv6 options are unchecked"); return; }
        assert_eq!(status, OK);
        let ((stream, stream_at), (datagram, datagram_at)) = (bound(V6, TCP), bound(V6, UDP));
        let hops = (option(stream, sockets::HOPS), option(datagram, sockets::HOPS));
        assert!((1..=255).contains(&hops.0) && hops.0 == hops.1, "{hops:?}");
        round_trip(stream, sockets::HOPS, 33, 33, 33);
        round_trip(datagram, sockets::HOPS, 1, 1, 1);
        round_trip(datagram, sockets::HOPS, 44, 44, 44);
        assert_eq!(MULTICAST.map(|name| option(datagram, name)), [1, 1, 0]);
        round_trip(datagram, sockets::MULTICAST_HOPS, 7, 7, 7);
        round_trip(datagram, sockets::MULTICAST_LOOPBACK, 0, 0, 0);
        assert_eq!((set_option(datagram, sockets::MULTICAST_INTERFACE, 999_999), option(datagram, sockets::MULTICAST_INTERFACE)), (ADDRESS_NOT_AVAILABLE, 0));
        unsupported(stream, MULTICAST);
        unsupported(datagram, KEEP_ALIVE_TUNING);
        round_trip(stream, sockets::KEEP_ALIVE_IDLE, 77, 77, 77);
        #[cfg(unix)]
        {
            assert_eq!(hops.0 as i32, kernel::default_hops_v6());
            let lo = kernel::loopback_index() as u64;
            round_trip(datagram, sockets::MULTICAST_INTERFACE, lo, lo, lo);
            let (fd, datagram_fd) = (kernel::descriptor(true, true, stream_at.port), kernel::descriptor(true, false, datagram_at.port));
            let held = |fd, level, name| kernel::option(fd, level, name).expect("getsockopt");
            assert_eq!((held(fd, libc::IPPROTO_IPV6, libc::IPV6_UNICAST_HOPS), held(datagram_fd, libc::IPPROTO_IPV6, libc::IPV6_UNICAST_HOPS), held(fd, libc::IPPROTO_TCP, kernel::KEEP_ALIVE_IDLE)), (33, 44, 77));
            assert_eq!([libc::IPV6_MULTICAST_HOPS, libc::IPV6_MULTICAST_LOOP, libc::IPV6_MULTICAST_IF].map(|name| held(datagram_fd, libc::IPPROTO_IPV6, name) as u64), [7, 0, lo]);
        }
        #[cfg(not(unix))]
        let _ = (stream_at, datagram_at);
        assert_eq!((close(stream), close(datagram)), (OK, OK));
    }

    #[test]
    fn sockets_datagrams_name_their_sender_and_truncate() {
        let _watchdog = watchdog("sockets_datagrams_name_their_sender_and_truncate");
        let ((a, a_at), (b, b_at)) = (bound(V4, UDP), bound(V4, UDP));
        let mut buffer = [0u8; 64];
        assert_eq!((send(a, b"datagram", Some(&b_at)), send(a, b"second one", Some(&b_at))), ((OK, 8), (OK, 10)));
        // AVAILABLE is the datagram a receive would return, not the queue behind it.
        assert_eq!((bits(b, READ, LIMIT * MS), option(b, sockets::AVAILABLE)), (READ, 8));
        assert_eq!(receive(b, &mut buffer, RECEIVE_PEEK), (OK, 8, a_at));
        buffer.fill(0);
        assert_eq!(receive(b, &mut buffer, 0), (OK, 8, a_at));
        assert_eq!(&buffer[..8], b"datagram");
        // A datagram longer than the buffer is cut to it and the rest is gone.
        assert_eq!(bits(b, READ, LIMIT * MS), READ);
        assert_eq!(receive(b, &mut buffer[..4], 0), (OK, 4, a_at));
        assert_eq!((&buffer[..4], bits(b, READ, 0)), (&b"seco"[..], 0));
        // An empty datagram is a real message with a sender.
        assert_eq!(send(b, &[], Some(&a_at)), (OK, 0));
        assert_eq!(bits(a, READ, LIMIT * MS), READ);
        assert_eq!(receive(a, &mut buffer, 0), (OK, 0, b_at));
        // A connected datagram socket needs no destination.
        assert_eq!((connect(a, &b_at), send(a, b"c", None)), (OK, (OK, 1)));
        assert_eq!((receive(b, &mut buffer, 0), buffer[0]), ((OK, 1, a_at), b'c'));
        assert_eq!(send(b, &vec![0u8; 70000], Some(&a_at)), (MESSAGE_TOO_LARGE, 0));
        assert_eq!(set_blocking(b, 0), OK);
        assert_eq!(receive(b, &mut buffer, 0), (WOULD_BLOCK, 0, Address::default()));
        assert_eq!((close(a), close(b)), (OK, OK));
    }

    #[test]
    fn sockets_ipv6_loopback_when_the_host_has_one() {
        let _watchdog = watchdog("sockets_ipv6_loopback_when_the_host_has_one");
        let (mut status, probe) = create(V6, TCP);
        if status == OK { status = bind(probe, &loopback(V6, 0)); assert_eq!(close(probe), OK); }
        if status == ADDRESS_NOT_AVAILABLE || status == UNSUPPORTED { println!("SOCKETS note: no IPv6 loopback here (status {status}), IPv6 checks skipped"); return; }
        assert_eq!(status, OK);
        let server = open(V6, TCP);
        round_trip(server, sockets::IPV6_ONLY, 1, 1, 1);
        assert_eq!((bind(server, &loopback(V6, 0)), unsafe { ops().listen.unwrap()(server, 8) }), (OK, OK));
        let at = local(server);
        assert!(at.port != 0);
        assert_eq!(at, loopback(V6, at.port));
        let (client, accepted) = connected(server, &at);
        transfer(client, accepted, b"six");
        transfer(accepted, client, b"xis");
        assert_eq!((close(client), close(accepted), close(server)), (OK, OK, OK));
        let ((a, a_at), b, mut buffer) = (bound(V6, UDP), open(V6, UDP), [0u8; 8]);
        assert_eq!(send(b, b"u6", Some(&a_at)), (OK, 2));
        assert_eq!(bits(a, READ, LIMIT * MS), READ);
        let (code, done, from) = receive(a, &mut buffer, 0);
        assert_eq!((code, done, &buffer[..2], from), (OK, 2, &b"u6"[..], loopback(V6, local(b).port)));
        assert_eq!((close(a), close(b)), (OK, OK));
    }

    #[test]
    fn sockets_send_to_a_closed_peer_is_a_status_not_a_signal() {
        let _watchdog = watchdog("sockets_send_to_a_closed_peer_is_a_status_not_a_signal");
        // The Rust runtime ignores SIGPIPE. With the default action back, a send that raised it would end this process.
        #[cfg(unix)]
        let previous = unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL) };
        let (server, at) = listener(V4);
        let (client, accepted) = connected(server, &at);
        assert_eq!(close(accepted), OK);
        // The first refusal may be the peer's reset, which raises nothing anywhere; the broken pipe after it is the send that would.
        let (mut first, mut last) = (OK, OK);
        for _ in 0..400 {
            let (code, done) = send(client, b"x", None);
            if code != OK {
                assert!((code == BROKEN_PIPE || code == CONNECTION_RESET) && done == 0, "status {code}, {done} bytes");
                if first == OK { first = code; }
                last = code;
                if code == BROKEN_PIPE { break; }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        #[cfg(unix)]
        unsafe { libc::signal(libc::SIGPIPE, previous) };
        assert_eq!(last, BROKEN_PIPE);
        println!("SOCKETS note: the first send to a closed peer reported {}", if first == BROKEN_PIPE { "BROKEN_PIPE" } else { "CONNECTION_RESET" });
        assert_eq!((close(client), close(server)), (OK, OK));
    }

    #[test]
    fn sockets_resolve_names_and_report_the_host_name() {
        let _watchdog = watchdog("sockets_resolve_names_and_report_the_host_name");
        let before = stats().resolve_ok;
        let mut found = [Address { family: 9, ..Address::default() }; 8];
        let (code, count) = resolve(b"localhost", 0, &mut found);
        assert!(code == OK && (1..=8).contains(&count), "status {code}, {count} addresses");
        assert!(found[..count].iter().any(|a| *a == loopback(V4, 0) || Address { scope: 0, ..*a } == loopback(V6, 0)), "{:?}", &found[..count]);
        assert!(found[count..].iter().all(|a| *a == Address::default()));
        for (i, address) in found[..count].iter().enumerate() { assert!(!found[..i].contains(address), "{address:?} listed twice"); }
        let (code, count) = resolve(b"localhost", V4, &mut found);
        assert!(code == OK && count >= 1 && found[..count].iter().all(|a| a.family as u32 == V4), "status {code}: {:?}", &found[..count]);
        assert_eq!((resolve(b"127.0.0.1", 0, &mut found[..1]), found[0]), ((OK, 1), loopback(V4, 0)));
        assert_eq!((resolve(b"::1", V6, &mut found[..2]), found[0]), ((OK, 1), loopback(V6, 0)));
        // A name under .invalid has no addresses. A resolver that cannot be reached cannot say so, which is TIMEOUT.
        found.fill(Address { family: 9, ..Address::default() });
        let (code, count) = resolve(b"no-such-host.dotnet-pal.invalid", 0, &mut found);
        if code == TIMEOUT { println!("SOCKETS note: the resolver gave no answer, NOT_FOUND for an unknown name is unchecked here"); } else { assert_eq!(code, NOT_FOUND); }
        assert_eq!((count, found[0]), (0, Address::default()));
        assert_eq!(resolve(b"localhost", 3, &mut found).0, INVALID_ARGUMENT);
        assert_eq!(resolve(b"local\0host", 0, &mut found).0, INVALID_ARGUMENT);
        assert!(stats().resolve_ok >= before + 4);

        let h = ops().host_name.unwrap();
        let (mut name, mut needed) = ([0xffu8; 300], 7usize);
        assert_eq!(unsafe { h(name.as_mut_ptr(), name.len(), &mut needed) }, OK);
        assert!(needed >= 2 && name[needed - 1] == 0 && !name[..needed - 1].contains(&0) && name[needed..].iter().all(|b| *b == 0), "needed {needed}");
        #[cfg(unix)]
        {
            let mut expected = [0u8; 300];
            assert_eq!(unsafe { libc::gethostname(expected.as_mut_ptr().cast(), expected.len() - 1) }, 0);
            assert_eq!(name[..needed], expected[..needed]);
        }
        let (mut short, mut again) = ([b'x'; 300], 7usize);
        assert_eq!(unsafe { h(short.as_mut_ptr(), needed - 1, &mut again) }, BUFFER_TOO_SMALL);
        assert!(again == needed && short[..needed - 1].iter().all(|b| *b == 0), "a short buffer is cleared, not half filled");
        assert_eq!((unsafe { h(ptr::null_mut(), 0, &mut again) }, again), (BUFFER_TOO_SMALL, needed));
        assert_eq!(unsafe { h(name.as_mut_ptr(), name.len(), ptr::null_mut()) }, INVALID_ARGUMENT);
    }

    #[test]
    fn sockets_reject_malformed_arguments_before_the_provider() {
        let s = ops();
        let before = stats();
        let (code, socket) = create(V4, UDP);
        assert_eq!(code, OK);
        let (at, bad, null) = (loopback(V4, 0), Address { family: 9, ..loopback(V4, 0) }, ptr::null_mut::<c_void>());
        for (family, kind) in [(0, TCP), (4, TCP), (V4, 0), (V4, 3)] { assert_eq!(create(family, kind), (INVALID_ARGUMENT, ptr::null_mut()), "family {family} kind {kind}"); }
        assert_eq!(unsafe { s.create.unwrap()(V4, TCP, ptr::null_mut()) }, INVALID_ARGUMENT);
        assert_eq!((close(null), bind(null, &at), bind(socket, &bad), unsafe { s.bind.unwrap()(socket, ptr::null()) }), (INVALID_ARGUMENT, INVALID_ARGUMENT, INVALID_ARGUMENT, INVALID_ARGUMENT));
        assert_eq!((unsafe { s.listen.unwrap()(null, 1) }, accept(null), connect(null, &at), connect(socket, &bad)),
            (INVALID_ARGUMENT, (INVALID_ARGUMENT, ptr::null_mut(), Address::default()), INVALID_ARGUMENT, INVALID_ARGUMENT));
        let mut buffer = [0u8; 8];
        assert_eq!((send(null, &buffer, None), send(socket, &buffer, Some(&bad))), ((INVALID_ARGUMENT, 0), (INVALID_ARGUMENT, 0)));
        assert_eq!(unsafe { s.send.unwrap()(socket, ptr::null(), 1, &at, &mut 0) }, INVALID_ARGUMENT);
        assert_eq!((receive(null, &mut buffer, 0), receive(socket, &mut buffer, 2)), ((INVALID_ARGUMENT, 0, Address::default()), (INVALID_ARGUMENT, 0, Address::default())));
        assert_eq!((shutdown(null, sockets::SHUTDOWN_BOTH), shutdown(socket, 0), shutdown(socket, 4)), (INVALID_ARGUMENT, INVALID_ARGUMENT, INVALID_ARGUMENT));
        assert_eq!((peer(null), set_blocking(null, 1), set_blocking(socket, 2)), ((INVALID_ARGUMENT, Address::default()), INVALID_ARGUMENT, INVALID_ARGUMENT));
        assert_eq!((get_option(null, sockets::ERROR), set_option(null, sockets::BROADCAST, 1)), ((INVALID_ARGUMENT, 0), INVALID_ARGUMENT));
        // poll: only READ and WRITE can be requested, and every entry names a socket.
        let mut entries = [PollEntry { socket, requested: READ, triggered: 99 }, PollEntry { socket, requested: ERROR, triggered: 99 }];
        assert_eq!((poll(&mut entries, 0, NO_CHANNEL), entries[0].triggered, entries[1].triggered), ((INVALID_ARGUMENT, 0), 0, 0));
        entries[1] = PollEntry { socket: null, requested: READ, triggered: 99 };
        assert_eq!(poll(&mut entries, 0, NO_CHANNEL), (INVALID_ARGUMENT, 0));
        assert_eq!(unsafe { s.poll.unwrap()(ptr::null_mut(), sockets::MAX_POLL + 1, 0, NO_CHANNEL, &mut 0) }, INVALID_ARGUMENT);
        assert_eq!(unsafe { s.read_stats.unwrap()(ptr::null_mut(), size_of::<sockets::Stats>()) }, INVALID_ARGUMENT);
        assert_eq!(close(socket), OK);
        let after = stats();
        assert!(after.rejected_or_failed >= before.rejected_or_failed + 25);
    }
}

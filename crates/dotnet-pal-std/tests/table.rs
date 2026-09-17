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

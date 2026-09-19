/* The negotiated dotnet_pal table, exercised on the bare-metal AArch64 port.
 *
 * Linked freestanding against the port archive and run by QEMU. There is no
 * libc: the only outside functions are the three the port exports (console
 * output, exit) and the C++ constructor probe. Anything this file needs beyond
 * that it does itself.
 *
 * Prints "TABLE PASS" and exits 0, or "FAIL <what>" and exits 1.
 */
#include "dotnet_pal.h"

extern void pal_console_write(const uint8_t *data, size_t size);
extern void pal_exit(int32_t code);
extern unsigned pal_constructor_value(void);
extern uint64_t pal_test_registers(uint64_t base, uint32_t (*yield)(void));
extern uint64_t pal_fault_load(const uint64_t *address);
extern uint64_t pal_fault_keeps(uint64_t *address);
extern const char pal_fault_keeps_store[];
extern size_t pal_region_start(void);
extern size_t pal_region_end(void);
extern size_t pal_region_free(void);

#define MIB (1024u * 1024u)
#define MS (UINT64_C(1000000))

static const dotnet_pal_api *api;
static const dotnet_pal_vm_ops *vm;
static const dotnet_pal_services_ops *svc;
static const dotnet_pal_kernel_ops *k;
static const dotnet_pal_runtime_ops *rt;
static const dotnet_pal_support_ops *sup;

static void put(const char *text) {
    size_t length = 0;
    while (text[length]) ++length;
    pal_console_write((const uint8_t *)text, length);
}
static void put_u64(uint64_t value) {
    char digits[24];
    int index = 24;
    if (value == 0) digits[--index] = '0';
    while (value != 0) { digits[--index] = (char)('0' + value % 10); value /= 10; }
    pal_console_write((const uint8_t *)&digits[index], (size_t)(24 - index));
}
static void fail(const char *what) {
    put("FAIL ");
    put(what);
    put("\n");
    pal_exit(1);
}
#define REQUIRE(condition, what) do { if (!(condition)) fail(what); } while (0)

static uint64_t now(void) {
    uint64_t value = 0;
    REQUIRE(svc->monotonic_ns(&value) == DOTNET_PAL_OK, "monotonic_ns");
    return value;
}
static void fill(unsigned char *bytes, size_t size, unsigned char value) {
    for (size_t i = 0; i < size; ++i) bytes[i] = value;
}
static int all(const unsigned char *bytes, size_t size, unsigned char value) {
    for (size_t i = 0; i < size; ++i) if (bytes[i] != value) return 0;
    return 1;
}

/* ELF thread-local storage: one initialized and one zero-initialized variable,
 * so the port has to copy .tdata and zero .tbss for every thread. */
static __thread int tls_initialized = 7;
static __thread int tls_zero;

static void *tls_key;          /* dynamic slot with a destructor */
static void *guard;            /* recursive mutex */
static void *ready_event;      /* auto-reset, signalled by workers */
static void *release_event;    /* manual-reset, signalled by main */
static volatile unsigned counted;
static volatile unsigned destroyed;
static volatile unsigned entered;
static volatile unsigned reader_entered;
static volatile uint64_t worker_thread_id;

static void destructor(void *value) {
    REQUIRE(value != NULL, "destructor value");
    ++destroyed;
}
static void check_stack(const char *what) {
    void *low = NULL, *high = NULL;
    unsigned char here;
    REQUIRE(k->stack_bounds(&low, &high) == DOTNET_PAL_OK, what);
    REQUIRE((uintptr_t)low < (uintptr_t)high, what);
    REQUIRE((uintptr_t)low <= (uintptr_t)&here && (uintptr_t)&here < (uintptr_t)high, what);
}

static void *worker(void *argument) {
    unsigned index = (unsigned)(uintptr_t)argument;
    void *value = (void *)1;

    REQUIRE(tls_initialized == 7, "thread-local .tdata copy");
    REQUIRE(tls_zero == 0, "thread-local .tbss zeroing");
    tls_initialized += (int)index;
    tls_zero = (int)index;

    REQUIRE(k->tls_get(tls_key, &value) == DOTNET_PAL_OK && value == NULL, "new thread slot is empty");
    REQUIRE(k->tls_set(tls_key, (void *)(uintptr_t)(index + 1)) == DOTNET_PAL_OK, "tls_set");
    REQUIRE(k->tls_get(tls_key, &value) == DOTNET_PAL_OK, "tls_get");
    REQUIRE(value == (void *)(uintptr_t)(index + 1), "tls value");

    uint64_t id = 0;
    REQUIRE(rt->thread_id(&id) == DOTNET_PAL_OK && id != 0, "thread_id");
    worker_thread_id = id;
    check_stack("worker stack bounds");

    const uint8_t name[] = "worker";
    REQUIRE(sup->thread_name(name, sizeof(name) - 1) == DOTNET_PAL_OK, "thread_name");

    uint64_t marker = UINT64_C(0x1122334455660000) + index;
    for (int i = 0; i < 1000; ++i) {
        REQUIRE(k->mutex_lock(guard) == DOTNET_PAL_OK, "worker mutex_lock");
        ++counted;
        REQUIRE(k->mutex_unlock(guard) == DOTNET_PAL_OK, "worker mutex_unlock");
        /* Interleave with the other workers, so the checks here and below really
         * do span context switches instead of one uninterrupted run. d8 is
         * x19-x28 and d8-d15 are callee-saved: the switch has to carry them
         * past the threads that run in between, each with its own marker. */
        if (i % 100 == 0) {
            REQUIRE(pal_test_registers(marker, svc->yield_thread) == 0,
                    "the callee-saved registers survive a context switch");
        }
    }
    /* The values above survived every switch this thread made. */
    REQUIRE(tls_initialized == 7 + (int)index, "thread-local stays per thread");
    REQUIRE(tls_zero == (int)index, "thread-local stays per thread");
    return NULL;
}
/* Counts itself in, waits to be released, then leaves. */
static void *waiter(void *argument) {
    (void)argument;
    ++entered;
    REQUIRE(k->event_wait(release_event, 5000 * MS) == DOTNET_PAL_OK, "waiter event_wait");
    ++counted;
    return NULL;
}
static void *detached_worker(void *argument) {
    (void)argument;
    REQUIRE(k->event_set(ready_event) == DOTNET_PAL_OK, "detached event_set");
    return NULL;
}
static void *reader_worker(void *argument) {
    REQUIRE(sup->rw_read(argument) == DOTNET_PAL_OK, "rw_read in thread");
    reader_entered = 1;
    REQUIRE(sup->rw_unlock(argument) == DOTNET_PAL_OK, "rw_unlock in thread");
    return NULL;
}

static void check_table(void) {
    const uint64_t required = DOTNET_PAL_CAP_VM | DOTNET_PAL_CAP_CLOCK | DOTNET_PAL_CAP_SCHEDULER
        | DOTNET_PAL_CAP_KERNEL | DOTNET_PAL_CAP_IDENTITY | DOTNET_PAL_CAP_REALTIME
        | DOTNET_PAL_CAP_NATIVE_MEMORY | DOTNET_PAL_CAP_SUPPORT | DOTNET_PAL_CAP_MODULES
        | DOTNET_PAL_CAP_TOPOLOGY | DOTNET_PAL_CAP_PROCESS | DOTNET_PAL_CAP_IMAGE | DOTNET_PAL_CAP_STREAMS
        | DOTNET_PAL_CAP_FILES | DOTNET_PAL_CAP_FAULTS;
    const uint64_t absent = DOTNET_PAL_CAP_LINEAR | DOTNET_PAL_CAP_ENVIRONMENT | DOTNET_PAL_CAP_ENTROPY
        | DOTNET_PAL_CAP_WASI_DISPATCH | DOTNET_PAL_CAP_NATIVE_CONTEXT | DOTNET_PAL_CAP_SOCKETS;
    REQUIRE(api != NULL, "dotnet_pal_get_api");
    REQUIRE(api->header.abi_version == DOTNET_PAL_ABI_VERSION, "abi version");
    REQUIRE(api->header.struct_size >= DOTNET_PAL_FAULTS_API_SIZE, "struct size");
    REQUIRE((api->header.capabilities & required) == required, "advertised capabilities");
    REQUIRE((api->header.capabilities & absent) == 0, "absent capabilities");
    /* An absent capability is a NULL callback, never a success stub. */
    REQUIRE(api->linear.allocate == NULL, "linear callbacks");
    REQUIRE(api->runtime.environment_get == NULL, "environment callback");
    REQUIRE(api->runtime.random_bytes == NULL, "entropy callback");
    REQUIRE(api->context.install == NULL, "context callbacks");
    REQUIRE(api->sockets.create == NULL && api->sockets.poll == NULL, "socket callbacks");
    /* The machine view: one CPU, the RAM figures, no debugger, the image's own tables, the serial streams. */
    uint32_t cpus = 0, present = 7, terminal = 0;
    REQUIRE(api->topology.cpu_count(&cpus) == DOTNET_PAL_OK && cpus == 1, "one cpu");
    uint64_t total = 0, available = 0;
    REQUIRE(api->topology.physical_memory(&total, &available) == DOTNET_PAL_OK && total > 0 && available <= total, "memory figures");
    REQUIRE(api->process.debugger_present(&present) == DOTNET_PAL_OK && present == 0, "no debugger");
    REQUIRE(api->process.crash_dump(NULL, 0, NULL, 0) == DOTNET_PAL_INVALID_ARGUMENT, "crash dump arguments");
    dotnet_pal_unwind_info unwind;
    REQUIRE(api->image.unwind_info((uintptr_t)&check_table, &unwind, sizeof unwind) == DOTNET_PAL_OK
            && unwind.text_start <= (uintptr_t)&check_table && unwind.eh_frame_hdr != 0, "image unwind tables");
    REQUIRE(api->image.readable((uintptr_t)&check_table, 8) == DOTNET_PAL_OK, "image readable");
    REQUIRE(api->image.readable(0x1000, 8) == DOTNET_PAL_NOT_FOUND, "device space is not readable");
    dotnet_pal_module_info module;
    REQUIRE(api->runtime.module_info((void*)&check_table, &module) == DOTNET_PAL_OK && module.name_length == 3, "module info");
    size_t written = 0, got = 5; uint8_t byte = 0;
    REQUIRE(api->streams.write(1, (const uint8_t*)"streams: this line went through stream 1\n", 41, &written) == DOTNET_PAL_OK && written == 41, "stream write");
    REQUIRE(api->streams.read(0, &byte, 1, &got) == DOTNET_PAL_OK && got == 0, "stream read is end of input");
    REQUIRE(api->streams.is_terminal(2, &terminal) == DOTNET_PAL_OK && terminal == 1, "stream terminal");
    REQUIRE(dotnet_pal_get_api(1) == NULL, "another ABI version is rejected");
    REQUIRE(dotnet_pal_get_api(DOTNET_PAL_ABI_VERSION) == api, "the table is negotiated once");
}

static void check_virtual_memory(void) {
    void *first = NULL, *second = NULL, *rejected = (void *)1;
    unsigned char *bytes;
    REQUIRE(vm->page_size() == 4096, "page size");
    REQUIRE(vm->reserve(0, 0, 0, &rejected) == DOTNET_PAL_INVALID_ARGUMENT && rejected == NULL, "reserve of nothing");
    rejected = (void *)1;
    REQUIRE(vm->reserve(4096, 3, 0, &rejected) == DOTNET_PAL_INVALID_ARGUMENT && rejected == NULL, "reserve alignment");
    rejected = (void *)1;
    REQUIRE(vm->reserve(4096, 0, 1, &rejected) == DOTNET_PAL_UNSUPPORTED && rejected == NULL, "reserve flags");

    REQUIRE(vm->reserve(MIB, 64 * 1024, 0, &first) == DOTNET_PAL_OK && first != NULL, "reserve");
    REQUIRE((uintptr_t)first % (64 * 1024) == 0, "reserve alignment honoured");
    REQUIRE(vm->reserve(MIB, 0, 0, &second) == DOTNET_PAL_OK && second != NULL, "second reserve");
    REQUIRE((uintptr_t)second + MIB <= (uintptr_t)first || (uintptr_t)first + MIB <= (uintptr_t)second,
            "live reservations do not overlap");

    REQUIRE(vm->commit(first, MIB) == DOTNET_PAL_OK, "commit");
    bytes = first;
    REQUIRE(all(bytes, MIB, 0), "committed memory reads as zero");
    fill(bytes, MIB, 0xa5);
    REQUIRE(vm->reset(bytes, 4096) == DOTNET_PAL_OK, "reset");
    REQUIRE(vm->decommit(bytes, 4096) == DOTNET_PAL_OK, "decommit");
    REQUIRE(all(bytes, 4096, 0), "decommit zeroes the range");
    REQUIRE(bytes[4096] == 0xa5 && bytes[MIB - 1] == 0xa5, "decommit leaves the neighbours alone");
    REQUIRE(vm->commit(bytes, 4096) == DOTNET_PAL_OK, "recommit");
    REQUIRE(all(bytes, 4096, 0), "recommitted memory reads as zero");

    /* Page rules are the front end's, and they hold for this provider too. */
    REQUIRE(vm->commit((unsigned char *)first + 1, 4096) == DOTNET_PAL_INVALID_ARGUMENT, "unaligned commit");
    REQUIRE(vm->decommit(NULL, 4096) == DOTNET_PAL_INVALID_ARGUMENT, "decommit of nothing");

    REQUIRE(vm->release(first, MIB) == DOTNET_PAL_OK, "release");
    REQUIRE(vm->release(second, MIB) == DOTNET_PAL_OK, "release");
    /* The region takes the memory back: the next reservation of the same shape fits. */
    REQUIRE(vm->reserve(2 * MIB, 0, 0, &first) == DOTNET_PAL_OK && first != NULL, "reserve after release");
    REQUIRE(all((unsigned char *)first, 2 * MIB, 0), "reused memory is zeroed");
    REQUIRE(vm->release(first, 2 * MIB) == DOTNET_PAL_OK, "release");
}

static void check_native_memory(void) {
    void *mapping = NULL;
    REQUIRE(rt->mapping_allocate(8192, DOTNET_PAL_READ | DOTNET_PAL_WRITE | DOTNET_PAL_EXECUTE, &mapping) == DOTNET_PAL_OK,
            "mapping_allocate");
    REQUIRE(mapping != NULL && (uintptr_t)mapping % 4096 == 0, "mapping alignment");
    REQUIRE(all(mapping, 8192, 0), "mapping starts zeroed");
    fill(mapping, 8192, 0x3c);
    REQUIRE(rt->mapping_protect(mapping, 8192, DOTNET_PAL_READ | DOTNET_PAL_EXECUTE) == DOTNET_PAL_OK, "mapping_protect");
    REQUIRE(rt->mapping_release(mapping, 8192) == DOTNET_PAL_OK, "mapping_release");
}

static void check_native_heap(void) {
    void *block = NULL, *bigger = NULL, *rejected = (void *)1;
    unsigned char *bytes;
    REQUIRE(sup->allocate(0, 0, &rejected) == DOTNET_PAL_INVALID_ARGUMENT && rejected == NULL, "allocate of nothing");
    REQUIRE(sup->allocate(100, 1, &block) == DOTNET_PAL_OK && block != NULL, "heap allocate");
    REQUIRE((uintptr_t)block % 16 == 0, "heap alignment");
    bytes = block;
    REQUIRE(all(bytes, 100, 0), "zeroed allocation");
    fill(bytes, 100, 0xc3);
    REQUIRE(sup->resize(block, 4000, &bigger) == DOTNET_PAL_OK && bigger != NULL, "heap resize");
    REQUIRE(all(bigger, 100, 0xc3), "resize keeps the contents");
    REQUIRE(sup->release(bigger) == DOTNET_PAL_OK, "heap release");
    REQUIRE(sup->release(NULL) == DOTNET_PAL_OK, "release of NULL is not an error");
    /* Repeated use has to reuse the freed blocks rather than run the slice down. */
    for (int round = 0; round < 4096; ++round) {
        void *churn = NULL;
        REQUIRE(sup->allocate(4096, 0, &churn) == DOTNET_PAL_OK && churn != NULL, "heap reuse");
        REQUIRE(sup->release(churn) == DOTNET_PAL_OK, "heap reuse release");
    }
}

static void check_time(void) {
    uint64_t start, stop, wall = 0;
    REQUIRE(now() <= now(), "the monotonic clock never goes back");
    start = now();
    while (now() == start) { /* the counter always advances */ }
    REQUIRE(rt->realtime_ns(&wall) == DOTNET_PAL_OK, "realtime_ns");
    REQUIRE(wall > UINT64_C(1767225600000000000), "realtime is at or after the port's epoch");
    start = now();
    REQUIRE(svc->sleep_ns(20 * MS) == DOTNET_PAL_OK, "sleep_ns");
    stop = now();
    REQUIRE(stop - start >= 19 * MS, "sleep waits for its deadline");
    REQUIRE(svc->yield_thread() == DOTNET_PAL_OK, "yield_thread");
    REQUIRE(k->process_barrier() == DOTNET_PAL_OK, "process_barrier");
}

static void check_events(void) {
    void *event = NULL;
    uint64_t start;
    REQUIRE(k->event_create(2, 0, &event) == DOTNET_PAL_INVALID_ARGUMENT, "event_create argument");
    REQUIRE(k->event_create(1, 1, &event) == DOTNET_PAL_OK && event != NULL, "manual event_create");
    REQUIRE(k->event_wait(event, 0) == DOTNET_PAL_OK, "manual event is signalled");
    REQUIRE(k->event_wait(event, 0) == DOTNET_PAL_OK, "manual event stays signalled");
    REQUIRE(k->event_reset(event) == DOTNET_PAL_OK, "event_reset");
    start = now();
    REQUIRE(k->event_wait(event, 20 * MS) == DOTNET_PAL_TIMEOUT, "event wait times out");
    REQUIRE(now() - start >= 19 * MS, "the timeout waits for its deadline");
    REQUIRE(k->event_destroy(event) == DOTNET_PAL_OK, "event_destroy");

    REQUIRE(k->event_create(0, 1, &event) == DOTNET_PAL_OK, "auto event_create");
    REQUIRE(k->event_wait(event, 0) == DOTNET_PAL_OK, "auto event is signalled");
    REQUIRE(k->event_wait(event, 0) == DOTNET_PAL_TIMEOUT, "auto event resets on the waiter");
    REQUIRE(k->event_set(event) == DOTNET_PAL_OK, "event_set");
    REQUIRE(k->event_set(event) == DOTNET_PAL_OK, "an event is not a counting semaphore");
    REQUIRE(k->event_wait(event, 0) == DOTNET_PAL_OK, "auto event after two sets");
    REQUIRE(k->event_wait(event, 0) == DOTNET_PAL_TIMEOUT, "auto event after two sets");
    REQUIRE(k->event_destroy(event) == DOTNET_PAL_OK, "event_destroy");
}

static void check_mutex(void) {
    REQUIRE(k->mutex_create(1, &guard) == DOTNET_PAL_OK && guard != NULL, "mutex_create");
    REQUIRE(k->mutex_lock(guard) == DOTNET_PAL_OK, "mutex_lock");
    REQUIRE(k->mutex_lock(guard) == DOTNET_PAL_OK, "recursive mutex_lock");
    REQUIRE(k->mutex_destroy(guard) == DOTNET_PAL_BUSY, "destroying a held mutex is refused");
    REQUIRE(k->mutex_unlock(guard) == DOTNET_PAL_OK, "mutex_unlock");
    REQUIRE(k->mutex_unlock(guard) == DOTNET_PAL_OK, "mutex_unlock");
}

static void check_threads(void) {
    void *threads[4];
    void *detached = NULL;
    void *value = (void *)1;
    uint64_t main_id = 0;

    REQUIRE(k->tls_create(destructor, &tls_key) == DOTNET_PAL_OK && tls_key != NULL, "tls_create");
    REQUIRE(rt->process_id(&main_id) == DOTNET_PAL_OK && main_id == 1, "process_id");
    REQUIRE(rt->thread_id(&main_id) == DOTNET_PAL_OK && main_id != 0, "thread_id");
    check_stack("main stack bounds");

    counted = 0;
    for (unsigned i = 0; i < 4; ++i) {
        REQUIRE(k->thread_create(worker, (void *)(uintptr_t)i, 64 * 1024, &threads[i]) == DOTNET_PAL_OK, "thread_create");
    }
    for (unsigned i = 0; i < 4; ++i) {
        REQUIRE(k->thread_join(threads[i]) == DOTNET_PAL_OK, "thread_join");
    }
    REQUIRE(counted == 4000, "every worker ran under the mutex");
    REQUIRE(destroyed == 4, "every worker ran its thread-local destructor");
    REQUIRE(worker_thread_id != main_id, "worker and main have different thread ids");
    REQUIRE(tls_initialized == 7 && tls_zero == 0, "the main thread kept its own thread-locals");
    REQUIRE(k->tls_get(tls_key, &value) == DOTNET_PAL_OK && value == NULL, "main never set the slot");

    /* A detached thread runs to the end and is never joined. */
    REQUIRE(k->event_create(0, 0, &ready_event) == DOTNET_PAL_OK, "ready event");
    for (int round = 0; round < 50; ++round) {
        REQUIRE(k->thread_create(detached_worker, NULL, 0, &detached) == DOTNET_PAL_OK, "detached thread_create");
        REQUIRE(k->thread_detach(detached) == DOTNET_PAL_OK, "thread_detach");
        REQUIRE(k->event_wait(ready_event, 5000 * MS) == DOTNET_PAL_OK, "detached thread ran");
    }

    /* Four threads blocked on one event, released together by a manual reset. */
    REQUIRE(k->event_create(1, 0, &release_event) == DOTNET_PAL_OK, "release event");
    counted = 0;
    entered = 0;
    for (unsigned i = 0; i < 4; ++i) {
        REQUIRE(k->thread_create(waiter, NULL, 0, &threads[i]) == DOTNET_PAL_OK, "waiter thread_create");
    }
    while (entered < 4) {
        REQUIRE(svc->yield_thread() == DOTNET_PAL_OK, "yield to the waiters");
    }
    REQUIRE(counted == 0, "the waiters are blocked on the event");
    REQUIRE(k->event_set(release_event) == DOTNET_PAL_OK, "release the waiters");
    for (unsigned i = 0; i < 4; ++i) {
        REQUIRE(k->thread_join(threads[i]) == DOTNET_PAL_OK, "waiter thread_join");
    }
    REQUIRE(counted == 4, "every waiter woke up");
    REQUIRE(k->event_destroy(ready_event) == DOTNET_PAL_OK, "event_destroy");
    REQUIRE(k->event_destroy(release_event) == DOTNET_PAL_OK, "event_destroy");

    REQUIRE(k->tls_destroy(tls_key) == DOTNET_PAL_OK, "tls_destroy");
    REQUIRE(destroyed == 4, "deleting a slot runs no destructor");
    REQUIRE(k->mutex_destroy(guard) == DOTNET_PAL_OK, "mutex_destroy");
}

static void check_rwlock(void) {
    void *lock = NULL, *thread = NULL;
    REQUIRE(sup->rw_create(&lock) == DOTNET_PAL_OK && lock != NULL, "rw_create");
    REQUIRE(sup->rw_read(lock) == DOTNET_PAL_OK, "rw_read");
    REQUIRE(sup->rw_read(lock) == DOTNET_PAL_OK, "a second read lock");
    REQUIRE(sup->rw_destroy(lock) == DOTNET_PAL_BUSY, "destroying a held lock is refused");
    REQUIRE(sup->rw_unlock(lock) == DOTNET_PAL_OK, "rw_unlock");
    REQUIRE(sup->rw_unlock(lock) == DOTNET_PAL_OK, "rw_unlock");
    REQUIRE(sup->rw_write(lock) == DOTNET_PAL_OK, "rw_write");

    /* A reader must wait while this thread holds the lock for writing. */
    reader_entered = 0;
    REQUIRE(k->thread_create(reader_worker, lock, 0, &thread) == DOTNET_PAL_OK, "reader thread_create");
    REQUIRE(svc->sleep_ns(10 * MS) == DOTNET_PAL_OK, "sleep_ns");
    REQUIRE(reader_entered == 0, "the reader waits for the writer");
    REQUIRE(sup->rw_unlock(lock) == DOTNET_PAL_OK, "rw_unlock");
    REQUIRE(k->thread_join(thread) == DOTNET_PAL_OK, "reader thread_join");
    REQUIRE(reader_entered == 1, "the reader got the lock");
    REQUIRE(sup->rw_destroy(lock) == DOTNET_PAL_OK, "rw_destroy");
}

static void check_diagnostics(void) {
    static const uint8_t message[] = "diagnostics: the port wrote this line through write_stderr\n";
    static const uint8_t name[] = "0123456789abcdef";
    size_t written = 12345;
    REQUIRE(sup->write_stderr(message, sizeof(message) - 1, &written) == DOTNET_PAL_OK, "write_stderr");
    REQUIRE(written == sizeof(message) - 1, "write_stderr wrote everything");
    REQUIRE(sup->write_stderr(NULL, 0, &written) == DOTNET_PAL_OK && written == 0, "an empty write");
    REQUIRE(sup->thread_name(name, 6) == DOTNET_PAL_OK, "thread_name");
    REQUIRE(sup->thread_name(name, 16) == DOTNET_PAL_INVALID_ARGUMENT, "a thread name over 15 bytes");
}

/* ---- faults: the exception vector reports, the handler edits the frame, the code resumes ---- */
static const char fault_token[] = "fault data";
static struct { uint32_t kind; uintptr_t address, pc, sp; void *data; int count; } seen;
static volatile double fault_scratch = 3.25;
static uint32_t on_fault(uint32_t kind, uintptr_t address, void *raw, size_t size, void *data) {
    dotnet_pal_fault_frame_arm64 *frame = raw;
    if (size != sizeof *frame) return DOTNET_PAL_FAULT_UNHANDLED;
    seen.kind = kind; seen.address = address; seen.pc = (uintptr_t)frame->pc; seen.sp = (uintptr_t)frame->sp; seen.data = data; seen.count++;
    /* Floating point work in the handler: the interrupted code must not notice it. */
    fault_scratch = fault_scratch * 1.5 + (double)seen.count;
    if (frame->pc == (uintptr_t)&pal_fault_load) frame->x[0] = UINT64_C(0x600df00d) + address;
    else if (frame->pc != (uintptr_t)pal_fault_keeps_store) return DOTNET_PAL_FAULT_UNHANDLED;
    frame->pc += 4; /* step over the instruction that faulted */
    return DOTNET_PAL_FAULT_RESUME;
}
static void *fault_worker(void *argument) {
    (void)argument;
    void *low = NULL, *high = NULL;
    REQUIRE(k->stack_bounds(&low, &high) == DOTNET_PAL_OK, "fault thread stack bounds");
    REQUIRE(pal_fault_load((const uint64_t *)0x18) == UINT64_C(0x600df00d) + 0x18, "fault resumed on a second thread");
    REQUIRE(seen.sp > (uintptr_t)low && seen.sp <= (uintptr_t)high, "the frame is on the faulting thread's stack");
    return NULL;
}
static void check_faults(void) {
    const dotnet_pal_faults_ops *faults = &api->faults;
    REQUIRE(faults->frame_tag() == DOTNET_PAL_FRAME_ARM64 && faults->frame_size() == sizeof(dotnet_pal_fault_frame_arm64), "fault frame layout");
    REQUIRE(faults->install(NULL, NULL) == DOTNET_PAL_INVALID_ARGUMENT, "install without a handler");
    REQUIRE(faults->install(on_fault, (void *)fault_token) == DOTNET_PAL_OK, "install the fault handler");
    REQUIRE(faults->install(on_fault, NULL) == DOTNET_PAL_BUSY, "a second install");
    /* The null page is unmapped: a load through a small address faults, and the handler supplies the value. */
    REQUIRE(pal_fault_load((const uint64_t *)0) == UINT64_C(0x600df00d), "null load resumed with the handler's value");
    REQUIRE(seen.count == 1 && seen.kind == DOTNET_PAL_FAULT_ACCESS && seen.address == 0, "fault kind and address");
    REQUIRE(seen.pc == (uintptr_t)&pal_fault_load && seen.data == (void *)fault_token, "fault pc and data");
    REQUIRE(pal_fault_load((const uint64_t *)0xff8) == UINT64_C(0x600df00d) + 0xff8 && seen.address == 0xff8, "fault address near the top of the null page");
    /* Caller-saved integer and FP registers survive a handler that uses both. */
    REQUIRE(pal_fault_keeps((uint64_t *)8) == 0 && seen.count == 3 && seen.address == 8, "registers across a resumed fault");
    void *thread = NULL;
    REQUIRE(k->thread_create(fault_worker, NULL, 0, &thread) == DOTNET_PAL_OK && k->thread_join(thread) == DOTNET_PAL_OK, "fault thread");
    dotnet_pal_faults_stats stats = {0};
    REQUIRE(faults->read_stats(&stats, sizeof stats) == DOTNET_PAL_OK, "fault read_stats");
    REQUIRE(stats.installs == 1 && stats.delivered == 4 && stats.resumed == 4 && stats.unhandled == 0 && stats.rejected == 2, "fault counters");
}

/* ---- files: the in-memory file system, living on the port's heap through the Rust allocator ---- */
#define PATH(text) (const uint8_t *)(text), sizeof(text) - 1
static void check_files(void) {
    const dotnet_pal_files_ops *f = &api->files;
    uint8_t cwd[8]; size_t needed = 0;
    REQUIRE(f->current_directory(cwd, sizeof cwd, &needed) == DOTNET_PAL_OK && needed == 2 && cwd[0] == '/' && cwd[1] == 0, "working directory");
    REQUIRE(f->directory_create(PATH("/data"), 0755) == DOTNET_PAL_OK, "create a directory");
    REQUIRE(f->directory_create(PATH("/data"), 0755) == DOTNET_PAL_ALREADY_EXISTS, "create it again");
    void *file = NULL, *missing = (void *)1;
    REQUIRE(f->open(PATH("/data/none"), DOTNET_PAL_FILE_READ, 0, &missing) == DOTNET_PAL_NOT_FOUND && missing == NULL, "open a missing file");
    REQUIRE(f->open(PATH("/data"), DOTNET_PAL_FILE_READ, 0, &missing) == DOTNET_PAL_IS_DIRECTORY, "open a directory");
    REQUIRE(f->open(PATH("/data/log.bin"), DOTNET_PAL_FILE_READ | DOTNET_PAL_FILE_WRITE | DOTNET_PAL_FILE_CREATE | DOTNET_PAL_FILE_EXCLUSIVE, 0644, &file) == DOTNET_PAL_OK && file, "create a file");
    /* A megabyte in pages, read back whole: the content lives in heap blocks the Rust allocator asked for. */
    static uint8_t page[4096], back[4096];
    size_t done = 0;
    for (uint32_t i = 0; i < 256; ++i) {
        fill(page, sizeof page, (unsigned char)(i + 1));
        REQUIRE(f->write_at(file, (uint64_t)i * sizeof page, page, sizeof page, &done) == DOTNET_PAL_OK && done == sizeof page, "write a page");
    }
    dotnet_pal_file_status status;
    REQUIRE(f->status(file, &status, sizeof status) == DOTNET_PAL_OK && status.kind == DOTNET_PAL_NODE_FILE && status.size == 256 * sizeof page, "file status");
    REQUIRE(status.mode == 0644 && status.identity != 0 && status.modified_ns >= UINT64_C(1767225600000000000), "mode, identity and a time from the port's clock");
    for (uint32_t i = 0; i < 256; i += 85) {
        REQUIRE(f->read_at(file, (uint64_t)i * sizeof page, back, sizeof back, &done) == DOTNET_PAL_OK && done == sizeof back, "read a page");
        REQUIRE(back[0] == (uint8_t)(i + 1) && back[sizeof back - 1] == (uint8_t)(i + 1), "page content");
    }
    REQUIRE(f->read_at(file, 256 * sizeof page, back, sizeof back, &done) == DOTNET_PAL_OK && done == 0, "end of file");
    REQUIRE(f->set_size(file, 10) == DOTNET_PAL_OK && f->flush(file) == DOTNET_PAL_OK, "truncate and flush");
    REQUIRE(f->rename(PATH("/data/log.bin"), PATH("/data/kept.bin")) == DOTNET_PAL_OK, "rename");
    REQUIRE(f->path_status(PATH("/data/kept.bin"), 1, &status, sizeof status) == DOTNET_PAL_OK && status.size == 10, "status by path");
    REQUIRE(f->path_status(PATH("/data/log.bin"), 1, &status, sizeof status) == DOTNET_PAL_NOT_FOUND, "the old name is gone");
    void *directory = NULL; uint8_t name[DOTNET_PAL_MAX_ENTRY_NAME]; size_t length = 0; uint32_t kind = 0;
    REQUIRE(f->directory_open(PATH("/data"), &directory) == DOTNET_PAL_OK && directory, "open the directory");
    REQUIRE(f->directory_read(directory, name, sizeof name, &length, &kind) == DOTNET_PAL_OK && length == 8 && name[0] == 'k' && kind == DOTNET_PAL_NODE_FILE, "the one entry");
    REQUIRE(f->directory_read(directory, name, sizeof name, &length, &kind) == DOTNET_PAL_NOT_FOUND, "end of the directory");
    REQUIRE(f->directory_close(directory) == DOTNET_PAL_OK, "close the directory");
    REQUIRE(f->directory_remove(PATH("/data")) == DOTNET_PAL_NOT_EMPTY, "remove a directory with an entry");
    REQUIRE(f->close(file) == DOTNET_PAL_OK && f->remove(PATH("/data/kept.bin")) == DOTNET_PAL_OK, "close and remove the file");
    REQUIRE(f->directory_remove(PATH("/data")) == DOTNET_PAL_OK, "remove the empty directory");
    REQUIRE(f->open(NULL, 1, DOTNET_PAL_FILE_READ, 0, &missing) == DOTNET_PAL_INVALID_ARGUMENT, "a null path");
    dotnet_pal_files_stats stats = {0};
    REQUIRE(f->read_stats(&stats, sizeof stats) == DOTNET_PAL_OK && stats.write_ok == 256 && stats.open_ok == 1 && stats.rejected_or_failed >= 6, "file counters");
}

int main(int argc, char **argv) {
    REQUIRE(argc == 1, "argc");
    REQUIRE(argv != NULL && argv[0] != NULL && argv[1] == NULL, "argv");
    REQUIRE(argv[0][0] == 'a' && argv[0][1] == 'p' && argv[0][2] == 'p' && argv[0][3] == 0, "argv[0]");
    REQUIRE(pal_constructor_value() == 0x5a5b, "the boot code ran .init_array");
    REQUIRE(tls_initialized == 7 && tls_zero == 0, "the main thread has its thread-locals");

    api = dotnet_pal_get_api(DOTNET_PAL_ABI_VERSION);
    check_table();
    vm = &api->vm;
    svc = &api->services;
    k = &api->kernel;
    rt = &api->runtime;
    sup = &api->support;

    check_virtual_memory();
    check_native_memory();
    check_native_heap();
    check_time();
    check_events();
    check_mutex();
    check_threads();
    check_rwlock();
    check_diagnostics();
    check_faults();
    check_files();

    dotnet_pal_kernel_stats kernel = {0};
    dotnet_pal_stats memory = {0};
    dotnet_pal_support_stats support = {0};
    REQUIRE(k->read_stats(&kernel, sizeof(kernel)) == DOTNET_PAL_OK, "kernel read_stats");
    REQUIRE(api->read_stats(&memory, sizeof(memory)) == DOTNET_PAL_OK, "vm read_stats");
    REQUIRE(sup->read_stats(&support, sizeof(support)) == DOTNET_PAL_OK, "support read_stats");
    REQUIRE(kernel.thread_create_ok == 60, "thread_create count");
    REQUIRE(kernel.mutex_lock_ok >= 4002, "mutex_lock count");
    REQUIRE(kernel.event_timeout >= 3, "event timeout count");
    REQUIRE(memory.reserve_ok >= 3 && memory.release_ok >= 3, "reserve and release counts");
    REQUIRE(support.allocate_ok >= 4097, "heap allocate count");
    /* Every reservation, mapping, thread stack and thread TLS block came back.
     * What is still out of the region is the native heap's 64 MiB slice and the
     * one page holding the boot thread's own TLS block, which it keeps. */
    REQUIRE(pal_region_end() - pal_region_start() - pal_region_free() == 64 * MIB + 4096,
            "the region got all of its memory back");

    put("TABLE PASS threads=");
    put_u64(kernel.thread_create_ok);
    put(" waits=");
    put_u64(kernel.event_wait_ok);
    put(" timeouts=");
    put_u64(kernel.event_timeout);
    put(" locks=");
    put_u64(kernel.mutex_lock_ok);
    put(" reserves=");
    put_u64(memory.reserve_ok);
    put(" heap=");
    put_u64(support.allocate_ok);
    put(" rejected=");
    put_u64(kernel.rejected_or_failed + memory.rejected_or_failed + support.rejected);
    put(" region=");
    put_u64((uint64_t)pal_region_start());
    put("..");
    put_u64((uint64_t)pal_region_end());
    put(" free=");
    put_u64((uint64_t)pal_region_free());
    put("\n");
    return 0;
}

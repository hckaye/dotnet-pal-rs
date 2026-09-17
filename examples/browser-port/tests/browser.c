/* C -> Rust -> JavaScript host contract for the browser profile. Linked
 * freestanding into a wasm32 module and executed by Node and by a real browser.
 * The return value is the failing line, 0 on success.
 * Compile with -DPAL_BROWSER_HEAP=1 for the grow (memory.grow) variant.
 */
#include "dotnet_pal.h"
#define REQUIRE(x) do { if (!(x)) return __LINE__; } while (0)
#define MIB (1024u * 1024u)
#ifndef PAL_BROWSER_HEAP
#define PAL_BROWSER_HEAP 0
#endif

static const uint8_t NAME_VALUE[] = "PAL_BROWSER_VALUE";
static const uint8_t NAME_EMPTY[] = "PAL_BROWSER_EMPTY";
static const uint8_t NAME_MISSING[] = "PAL_BROWSER_MISSING";
static const uint8_t NAME_BAD[] = "PAL=BROWSER";
static const uint8_t MESSAGE[] = "browser diagnostics\n";

static int storage_contract(const dotnet_pal_api *a) {
    const dotnet_pal_linear_ops *l = &a->linear;
    size_t g = l->granularity();
    REQUIRE(g == 4096 && l->capacity() == 8 * MIB);
    void *p = (void *)1;
    REQUIRE(l->allocate(2 * g, 0, 0, &p) == DOTNET_PAL_OK && p != NULL);
    unsigned char *bytes = p;
    for (size_t i = 0; i < 2 * g; ++i) REQUIRE(bytes[i] == 0);
    bytes[0] = 0x5a; bytes[2 * g - 1] = 0xa5;
    REQUIRE(l->zero(bytes, g) == DOTNET_PAL_OK && bytes[0] == 0 && bytes[2 * g - 1] == 0xa5);
    REQUIRE(l->release(bytes + g, g) == DOTNET_PAL_INVALID_ARGUMENT);
    REQUIRE(l->release(p, 2 * g) == DOTNET_PAL_OK);
#if PAL_BROWSER_HEAP
    /* Demand storage: 3 MiB must grow the instance memory (observed by the host),
     * more than the 8 MiB budget fails before the provider, and a request that the
     * linker-imposed 6 MiB maximum cannot satisfy fails inside memory.grow. */
    void *big = NULL, *again = NULL, *huge = (void *)1;
    REQUIRE(l->allocate(3 * MIB, 0, 0, &big) == DOTNET_PAL_OK && big != NULL);
    bytes = big;
    for (size_t i = 0; i < 3 * MIB; i += g) REQUIRE(bytes[i] == 0);
    for (size_t i = 0; i < 3 * MIB; i += g) bytes[i] = (unsigned char)(i >> 12);
    REQUIRE(l->allocate(16 * MIB, 0, 0, &huge) == DOTNET_PAL_OUT_OF_MEMORY && huge == NULL);
    REQUIRE(l->allocate(4 * MIB, 0, 0, &huge) == DOTNET_PAL_OUT_OF_MEMORY && huge == NULL);
    for (size_t i = 0; i < 3 * MIB; i += g) REQUIRE(bytes[i] == (unsigned char)(i >> 12)); /* failures had no side effects */
    REQUIRE(l->release(big, 3 * MIB) == DOTNET_PAL_OK);
    REQUIRE(l->allocate(2 * MIB, 0, 0, &again) == DOTNET_PAL_OK && again == big); /* first-fit reuse of the freed block */
    for (size_t i = 0; i < 2 * MIB; i += g) REQUIRE(((unsigned char *)again)[i] == 0);
    REQUIRE(l->release(again, 2 * MIB) == DOTNET_PAL_OK);
#endif
    return 0;
}

int pal_browser_test(int mode) {
    const dotnet_pal_api *a = dotnet_pal_get_api(DOTNET_PAL_ABI_VERSION);
    REQUIRE(a && a->header.abi_version == 2 && a->header.struct_size >= DOTNET_PAL_SUPPORT_API_SIZE);
    REQUIRE(dotnet_pal_get_api(1) == NULL);
    uint64_t expected = DOTNET_PAL_CAP_LINEAR | DOTNET_PAL_CAP_CLOCK | DOTNET_PAL_CAP_ENVIRONMENT |
        DOTNET_PAL_CAP_REALTIME | DOTNET_PAL_CAP_ENTROPY | DOTNET_PAL_CAP_DIAGNOSTICS;
    if (PAL_BROWSER_HEAP) expected |= DOTNET_PAL_CAP_DYNAMIC_LINEAR;
    REQUIRE(a->header.capabilities == expected);
    /* Absent services are NULL, never success stubs. */
    REQUIRE(!a->vm.reserve && !a->vm.commit && !a->vm.page_size);
    REQUIRE(!a->services.sleep_ns && !a->services.yield_thread);
    REQUIRE(!a->kernel.event_create && !a->kernel.mutex_create && !a->kernel.thread_create && !a->kernel.tls_create);
    REQUIRE(!a->runtime.process_id && !a->runtime.thread_id && !a->runtime.mapping_allocate && !a->runtime.module_open);
    REQUIRE(!a->support.allocate && !a->support.resize && !a->support.rw_create && !a->support.thread_name);
    REQUIRE(!a->wasi.invoke && !a->context.install);
    REQUIRE(a->services.monotonic_ns && a->runtime.environment_get && a->runtime.realtime_ns &&
        a->runtime.random_bytes && a->support.write_stderr && a->linear.allocate);
    const dotnet_pal_services_ops *s = &a->services;
    const dotnet_pal_runtime_ops *r = &a->runtime;
    const dotnet_pal_support_ops *d = &a->support;
    uint64_t t0 = 99, t1 = 99;
    uint8_t value[32];
    size_t needed = 99, written = 99;
    for (int i = 0; i < 32; ++i) value[i] = 42;

    if (mode == 1) { /* host scribbles into the output and reports OS_ERROR */
        REQUIRE(s->monotonic_ns(&t0) == DOTNET_PAL_OS_ERROR && t0 == 0);
        REQUIRE(r->realtime_ns(&t1) == DOTNET_PAL_OS_ERROR && t1 == 0);
        return 0;
    }
    if (mode == 2) { /* host fills partial entropy then fails: output is zeroed */
        REQUIRE(r->random_bytes(value, sizeof value) == DOTNET_PAL_OS_ERROR);
        for (int i = 0; i < 32; ++i) REQUIRE(value[i] == 0);
        return 0;
    }
    if (mode == 3) { /* unknown host status must become OS_ERROR with sanitized outputs */
        REQUIRE(r->environment_get(NAME_VALUE, sizeof NAME_VALUE - 1, value, sizeof value, &needed) == DOTNET_PAL_OS_ERROR);
        REQUIRE(needed == 0 && value[0] == 0);
        REQUIRE(s->monotonic_ns(&t0) == DOTNET_PAL_OS_ERROR && t0 == 0);
        return 0;
    }
    if (mode == 4) { /* short diagnostic write reported as success is a broken host */
        REQUIRE(d->write_stderr(MESSAGE, sizeof MESSAGE - 1, &written) == DOTNET_PAL_OS_ERROR && written == 0);
        return 0;
    }
    if (mode == 5) { /* OK with a required length above capacity is a broken host */
        REQUIRE(r->environment_get(NAME_VALUE, sizeof NAME_VALUE - 1, value, 2, &needed) == DOTNET_PAL_OS_ERROR);
        REQUIRE(needed == 0 && value[0] == 0);
        return 0;
    }
    REQUIRE(mode == 0);
    /* Clock: invalid outputs never reach the host; readings are nondecreasing. */
    REQUIRE(s->monotonic_ns(NULL) == DOTNET_PAL_INVALID_ARGUMENT);
    _Alignas(uint64_t) unsigned char misaligned[sizeof(uint64_t) + 1];
    REQUIRE(s->monotonic_ns((uint64_t *)(misaligned + 1)) == DOTNET_PAL_INVALID_ARGUMENT);
    REQUIRE(s->monotonic_ns(&t0) == DOTNET_PAL_OK);
    REQUIRE(s->monotonic_ns(&t1) == DOTNET_PAL_OK && t1 >= t0);
    /* Wall clock after 2000-01-01T00:00:00Z. */
    REQUIRE(r->realtime_ns(&t1) == DOTNET_PAL_OK && t1 > UINT64_C(946684800000000000));
    REQUIRE(r->realtime_ns(NULL) == DOTNET_PAL_INVALID_ARGUMENT);
    /* Entropy: chunked above the 64 KiB Web Crypto limit; zero-size never enters the host. */
    REQUIRE(r->random_bytes(value, sizeof value) == DOTNET_PAL_OK);
    { int nonzero = 0; for (int i = 0; i < 32; ++i) nonzero |= value[i]; REQUIRE(nonzero); }
    static uint8_t large[70000];
    REQUIRE(r->random_bytes(large, sizeof large) == DOTNET_PAL_OK);
    { int nonzero = 0; for (size_t i = 65536; i < sizeof large; ++i) nonzero |= large[i]; REQUIRE(nonzero); }
    REQUIRE(r->random_bytes(NULL, 0) == DOTNET_PAL_OK);
    REQUIRE(r->random_bytes(NULL, 1) == DOTNET_PAL_INVALID_ARGUMENT);
    /* Environment snapshot supplied by the page. */
    REQUIRE(r->environment_get(NAME_VALUE, sizeof NAME_VALUE - 1, NULL, 0, &needed) == DOTNET_PAL_BUFFER_TOO_SMALL && needed == 4);
    REQUIRE(r->environment_get(NAME_VALUE, sizeof NAME_VALUE - 1, value, sizeof value, &needed) == DOTNET_PAL_OK);
    REQUIRE(needed == 4 && value[0] == 'a' && value[1] == 'b' && value[2] == 'c' && value[3] == 0);
    REQUIRE(r->environment_get(NAME_EMPTY, sizeof NAME_EMPTY - 1, value, sizeof value, &needed) == DOTNET_PAL_OK && needed == 1 && value[0] == 0);
    REQUIRE(r->environment_get(NAME_MISSING, sizeof NAME_MISSING - 1, value, sizeof value, &needed) == DOTNET_PAL_NOT_FOUND && needed == 0);
    REQUIRE(r->environment_get(NAME_BAD, sizeof NAME_BAD - 1, value, sizeof value, &needed) == DOTNET_PAL_INVALID_ARGUMENT);
    /* Diagnostics reach the page's sink; zero-size and invalid requests do not. */
    REQUIRE(d->write_stderr(MESSAGE, sizeof MESSAGE - 1, &written) == DOTNET_PAL_OK && written == sizeof MESSAGE - 1);
    REQUIRE(d->write_stderr(NULL, 0, &written) == DOTNET_PAL_OK && written == 0);
    REQUIRE(d->write_stderr(MESSAGE, 1, NULL) == DOTNET_PAL_INVALID_ARGUMENT);
    { int line = storage_contract(a); if (line) return line; }
    /* Diagnostic counters: one per successful call, rejected calls counted separately. */
    dotnet_pal_services_stats cs; dotnet_pal_runtime_stats rs; dotnet_pal_support_stats ds; dotnet_pal_stats vm;
    REQUIRE(s->read_stats(&cs, sizeof cs) == DOTNET_PAL_OK && cs.clock_ok == 2 && cs.sleep_ok == 0 && cs.yield_ok == 0 && cs.rejected_or_failed == 2);
    REQUIRE(r->read_stats(&rs, sizeof rs) == DOTNET_PAL_OK && rs.environment_ok == 2 && rs.realtime_ok == 1 && rs.entropy_ok == 3 && rs.identity_ok == 0);
    REQUIRE(d->read_stats(&ds, sizeof ds) == DOTNET_PAL_OK && ds.write_ok == 2 && ds.allocate_ok == 0 && ds.rejected == 1);
    REQUIRE(a->read_stats(&vm, sizeof vm) == DOTNET_PAL_OK && vm.reserve_ok == 0 && vm.commit_ok == 0);
    return 0;
}

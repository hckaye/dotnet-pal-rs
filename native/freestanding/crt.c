/* CRT pieces with no OS behind them: errno storage, process end, exit
 * handlers, C++ ABI helpers with C linkage, stack protector data, the LSE
 * atomics flag for libgcc, and the AArch64 instruction-cache maintenance
 * that the JIT-less runtime still calls when it writes code stubs.
 */
#include "freestanding.h"

static _Thread_local int errno_value;
int *FS_NAME(__errno_location)(void) { return &errno_value; }

void *FS_NAME(__dso_handle) = &FS_NAME(__dso_handle);

/* Fixed nonzero canary: no entropy source runs before the port starts. */
uintptr_t FS_NAME(__stack_chk_guard) = (uintptr_t)0x595e9fbd94fda766ull;

/* libgcc's outline atomics read this byte; keeping it 0 selects the LL/SC
 * path and stops the linker pulling lse-init.o and its getauxval constructor. */
unsigned char FS_NAME(__aarch64_have_lse_atomics) = 0;

FS_NORETURN void FS_NAME(abort)(void) { dotnet_pal_freestanding_abort(); }
FS_NORETURN void FS_NAME(__stack_chk_fail)(void) { dotnet_pal_freestanding_abort(); }
FS_NORETURN void FS_NAME(__cxa_pure_virtual)(void) { dotnet_pal_freestanding_abort(); }
FS_NORETURN void FS_NAME(__cxa_deleted_virtual)(void) { dotnet_pal_freestanding_abort(); }
FS_NORETURN void FS_NAME(_Exit)(int status) { dotnet_pal_freestanding_exit(status); }

/* Exit handlers: a fixed table, run in reverse registration order. Handlers
 * registered while exit runs are appended and also run. */
#define FS_ATEXIT_MAX 64
typedef struct {
    union { void (*plain)(void); void (*cxa)(void *); } fn;
    void *arg, *dso;
    int is_cxa;
} exit_entry;
static exit_entry exit_table[FS_ATEXIT_MAX];
static int exit_count;

int FS_NAME(__cxa_atexit)(void (*fn)(void *), void *arg, void *dso) {
    if (!fn || exit_count >= FS_ATEXIT_MAX) return -1;
    exit_entry *e = &exit_table[exit_count++];
    e->fn.cxa = fn; e->arg = arg; e->dso = dso; e->is_cxa = 1;
    return 0;
}

int FS_NAME(atexit)(void (*fn)(void)) {
    if (!fn || exit_count >= FS_ATEXIT_MAX) return -1;
    exit_entry *e = &exit_table[exit_count++];
    e->fn.plain = fn; e->arg = NULL; e->dso = NULL; e->is_cxa = 0;
    return 0;
}

static void run_entry(exit_entry *e) {
    exit_entry copy = *e;
    e->fn.cxa = NULL; e->is_cxa = 1;
    if (!copy.fn.cxa) return;
    if (copy.is_cxa) copy.fn.cxa(copy.arg); else copy.fn.plain();
}

void FS_NAME(__cxa_finalize)(void *dso) {
    if (!dso) { while (exit_count > 0) run_entry(&exit_table[--exit_count]); return; }
    for (int i = exit_count - 1; i >= 0; --i) if (exit_table[i].dso == dso) run_entry(&exit_table[i]);
}

FS_NORETURN void FS_NAME(exit)(int status) {
    FS_NAME(__cxa_finalize)(NULL);
    dotnet_pal_freestanding_exit(status);
}

/* Function-local static guards (Itanium ABI, 64-bit guard on AArch64): byte 0
 * is "initialized", byte 1 is "in progress". Cooperative threads never wait
 * here; a second acquirer during initialization is a recursion bug and aborts. */
int FS_NAME(__cxa_guard_acquire)(uint64_t *guard) {
    unsigned char *b = (unsigned char *)guard;
    if (__atomic_load_n(&b[0], __ATOMIC_ACQUIRE)) return 0;
    if (b[1]) dotnet_pal_freestanding_abort();
    b[1] = 1;
    return 1;
}
void FS_NAME(__cxa_guard_release)(uint64_t *guard) {
    unsigned char *b = (unsigned char *)guard;
    b[1] = 0;
    __atomic_store_n(&b[0], 1, __ATOMIC_RELEASE);
}
void FS_NAME(__cxa_guard_abort)(uint64_t *guard) { ((unsigned char *)guard)[1] = 0; }

/* Makes stores in [begin, end) visible to instruction fetch: clean each data
 * cache line to the point of unification, invalidate the matching instruction
 * lines, then synchronize. CTR_EL0 gives the line sizes and the IDC/DIC bits
 * that make one of the two passes unnecessary. */
void FS_NAME(__clear_cache)(void *begin, void *end) {
#if defined(__aarch64__)
    uint64_t ctr;
    __asm__ volatile("mrs %0, ctr_el0" : "=r"(ctr));
    uintptr_t dline = (uintptr_t)4 << ((ctr >> 16) & 0xF), iline = (uintptr_t)4 << (ctr & 0xF);
    uintptr_t b = (uintptr_t)begin, e = (uintptr_t)end;
    if (!((ctr >> 28) & 1)) {
        for (uintptr_t a = b & ~(dline - 1); a < e; a += dline) __asm__ volatile("dc cvau, %0" : : "r"(a) : "memory");
    }
    __asm__ volatile("dsb ish" : : : "memory");
    if (!((ctr >> 29) & 1)) {
        for (uintptr_t a = b & ~(iline - 1); a < e; a += iline) __asm__ volatile("ic ivau, %0" : : "r"(a) : "memory");
        __asm__ volatile("dsb ish" : : : "memory");
    }
    __asm__ volatile("isb" : : : "memory");
#else
#error "__clear_cache is written for AArch64 only"
#endif
}

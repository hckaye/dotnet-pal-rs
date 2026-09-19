/* malloc family on the boundary's native heap (support.allocate/resize/
 * release). Every block carries a 16-byte header just below the returned
 * pointer holding the provider pointer, the requested size and the alignment,
 * so free and realloc never touch memory outside their own block. The
 * provider's memory is treated as 16-byte aligned, and the header math also
 * stays correct if it is only pointer aligned. Without a table (or without
 * DOTNET_PAL_CAP_NATIVE_HEAP) every allocation fails with ENOMEM.
 */
#include "freestanding.h"
#include "dotnet_pal.h"

#define FS_HEAP_ALIGN 16u
typedef struct { void *raw; uint64_t packed; } block;   /* packed = size | log2(align) << 56 */

_Static_assert(sizeof(block) == FS_HEAP_ALIGN, "header must keep 16-byte alignment");
void *memmove(void *d, const void *s, size_t n);

static const dotnet_pal_support_ops *heap_ops(void) {
    static const dotnet_pal_support_ops *ops;
    if (ops) return ops;
    const dotnet_pal_api *a = dotnet_pal_get_api(DOTNET_PAL_ABI_VERSION);
    if (!a || a->header.abi_version != DOTNET_PAL_ABI_VERSION || a->header.struct_size < DOTNET_PAL_SUPPORT_API_SIZE
        || !(a->header.capabilities & DOTNET_PAL_CAP_NATIVE_HEAP)
        || !a->support.allocate || !a->support.resize || !a->support.release) return NULL;
    ops = &a->support;
    return ops;
}

static int log2_of(size_t alignment) { int k = 0; while (((size_t)1 << k) < alignment) ++k; return k; }
static size_t block_size(const block *h) { return (size_t)(h->packed & ((1ull << 56) - 1u)); }
static size_t block_align(const block *h) { return (size_t)1 << (h->packed >> 56); }
static uint64_t pack(size_t size, size_t alignment) { return (uint64_t)size | ((uint64_t)log2_of(alignment) << 56); }

/* Total provider bytes for a payload of n bytes at the given alignment. */
static int total_size(size_t n, size_t alignment, size_t *total) {
    if (n > (SIZE_MAX >> 2) || alignment > (SIZE_MAX >> 2)) return 0;
    *total = n + sizeof(block) + (alignment - 1);
    return 1;
}

static void *place(void *raw, size_t n, size_t alignment) {
    uintptr_t p = ((uintptr_t)raw + sizeof(block) + (alignment - 1)) & ~(uintptr_t)(alignment - 1);
    block *h = (block *)p - 1;
    h->raw = raw; h->packed = pack(n, alignment);
    return (void *)p;
}

static void *allocate_block(size_t n, size_t alignment, int zero) {
    const dotnet_pal_support_ops *ops = heap_ops();
    size_t total; void *raw = NULL;
    if (!ops || !total_size(n ? n : 1, alignment, &total) || ops->allocate(total, zero ? 1u : 0u, &raw) != DOTNET_PAL_OK || !raw) {
        fs_errno = FS_ENOMEM;
        return NULL;
    }
    return place(raw, n, alignment);
}

void *FS_NAME(malloc)(size_t n) { return allocate_block(n, FS_HEAP_ALIGN, 0); }

void *FS_NAME(calloc)(size_t count, size_t size) {
    if (size && count > SIZE_MAX / size) { fs_errno = FS_ENOMEM; return NULL; }
    return allocate_block(count * size, FS_HEAP_ALIGN, 1);
}

void FS_NAME(free)(void *p) {
    if (!p) return;
    block *h = (block *)p - 1;
    (void)heap_ops()->release(h->raw);
}

void *FS_NAME(realloc)(void *p, size_t n) {
    if (!p) return FS_NAME(malloc)(n);
    if (!n) { FS_NAME(free)(p); return NULL; }
    block *h = (block *)p - 1;
    size_t old = block_size(h), alignment = block_align(h);
    if (alignment > FS_HEAP_ALIGN) {
        /* Over-aligned blocks are moved by hand; resize cannot keep the alignment. */
        void *q = allocate_block(n, alignment, 0);
        if (!q) return NULL;
        memcpy(q, p, old < n ? old : n);
        FS_NAME(free)(p);
        return q;
    }
    size_t total, offset = (size_t)((char *)p - (char *)h->raw); void *raw = NULL;
    if (!total_size(n, alignment, &total) || heap_ops()->resize(h->raw, total, &raw) != DOTNET_PAL_OK || !raw) {
        fs_errno = FS_ENOMEM;   /* the old block stays valid */
        return NULL;
    }
    uintptr_t q = ((uintptr_t)raw + sizeof(block) + (alignment - 1)) & ~(uintptr_t)(alignment - 1);
    if (q != (uintptr_t)raw + offset) memmove((void *)q, (char *)raw + offset, old < n ? old : n);
    block *nh = (block *)q - 1;
    nh->raw = raw; nh->packed = pack(n, alignment);
    return (void *)q;
}

static int power_of_two(size_t v) { return v && !(v & (v - 1)); }

int FS_NAME(posix_memalign)(void **out, size_t alignment, size_t n) {
    if (!power_of_two(alignment) || alignment % sizeof(void *)) return FS_EINVAL;
    void *p = allocate_block(n, alignment < FS_HEAP_ALIGN ? FS_HEAP_ALIGN : alignment, 0);
    if (!p) return FS_ENOMEM;
    *out = p;
    return 0;
}

/* Like glibc, n need not be a multiple of alignment. */
void *FS_NAME(aligned_alloc)(size_t alignment, size_t n) {
    if (!power_of_two(alignment)) { fs_errno = FS_EINVAL; return NULL; }
    return allocate_block(n, alignment < FS_HEAP_ALIGN ? FS_HEAP_ALIGN : alignment, 0);
}

void *FS_NAME(memalign)(size_t alignment, size_t n) { return FS_NAME(aligned_alloc)(alignment, n); }

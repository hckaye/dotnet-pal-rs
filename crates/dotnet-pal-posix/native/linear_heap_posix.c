/* Reference backend: one shared C allocator for native and managed storage.
 * WASI libc owns memory.grow/program-break coordination; free permits reuse but
 * does not promise that the WebAssembly engine can shrink physical memory.
 */
#define _POSIX_C_SOURCE 200809L
#include "dotnet_pal.h"
#include <errno.h>
#include <stdlib.h>
uint32_t dotnet_pal_storage_allocate_v2(size_t size, size_t alignment, void **out) {
    if (!out) return DOTNET_PAL_INVALID_ARGUMENT;
    *out = NULL;
    if (!size || alignment < sizeof(void*) || (alignment & (alignment - 1)))
        return DOTNET_PAL_INVALID_ARGUMENT;
    int rc = posix_memalign(out, alignment, size);
    if (rc == 0) return DOTNET_PAL_OK;
    *out = NULL;
    return rc == ENOMEM ? DOTNET_PAL_OUT_OF_MEMORY : DOTNET_PAL_OS_ERROR;
}
uint32_t dotnet_pal_storage_release_v2(void *address, size_t size) {
    if (!address || !size) return DOTNET_PAL_INVALID_ARGUMENT;
    free(address);
    return DOTNET_PAL_OK;
}

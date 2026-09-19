/* Conformance test of the image group on Linux: unwind tables of this executable,
 * readability probes against mapped and inaccessible pages, and the build id. */
#define _GNU_SOURCE
#include "dotnet_pal.h"
#include <assert.h>
#include <dlfcn.h>
#include <link.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>
#ifdef PAL_HOST_TEST
extern int pal_image_fault;
#endif
static const dotnet_pal_image_ops *im;
static int eh_frame_hdr(struct dl_phdr_info *info, size_t size, void *data) {
    (void)size;
    for (int i = 0; i < info->dlpi_phnum; ++i)
        if (info->dlpi_phdr[i].p_type == PT_GNU_EH_FRAME) { *(uintptr_t*)data = info->dlpi_addr + info->dlpi_phdr[i].p_vaddr; return 1; }
    return 0;
}
__attribute__((noinline)) static int probe_function(int x) { return x * 3 + 1; }
int main(int argc, char **argv) {
#ifdef PAL_HOST_TEST
    pal_image_fault = argc > 1 ? atoi(argv[1]) : 0;
#else
    (void)argc; (void)argv;
#endif
    const dotnet_pal_api *api = dotnet_pal_get_api(2);
#ifdef PAL_HOST_TEST
    if (pal_image_fault == 1) { assert(!api); puts("IMAGE malformed host rejected"); return 0; }
#endif
    assert(api && api->header.struct_size >= DOTNET_PAL_IMAGE_API_SIZE);
    assert(api->header.capabilities & DOTNET_PAL_CAP_IMAGE);
    im = &api->image;
    uintptr_t address = (uintptr_t)&probe_function;
    dotnet_pal_unwind_info info;
    memset(&info, 0x55, sizeof info);
#ifdef PAL_HOST_TEST
    if (pal_image_fault == 2) {
        /* A table that cannot contain the address is a broken provider: rejected, zeroed. */
        assert(im->unwind_info(address, &info, sizeof info) == DOTNET_PAL_OS_ERROR && info.base == 0 && info.eh_frame_hdr == 0);
        assert(im->readable(address, 8) == DOTNET_PAL_OS_ERROR);
        uint8_t id[8] = {1, 1, 1, 1, 1, 1, 1, 1}; size_t needed = 9;
        assert(im->build_id(address, id, sizeof id, &needed) == DOTNET_PAL_OS_ERROR && needed == 0 && id[0] == 0);
        puts("IMAGE host errors sanitized"); return 0;
    }
#endif
    assert(im->unwind_info(address, &info, sizeof info) == 0);
    assert(info.text_start <= address && address < info.text_start + info.text_length);
    uintptr_t expected_hdr = 0;
    assert(dl_iterate_phdr(eh_frame_hdr, &expected_hdr) == 1 && expected_hdr != 0);
    assert(info.eh_frame_hdr == expected_hdr && info.eh_frame_hdr_length > 0);
    dotnet_pal_unwind_info found = info;
    assert(im->unwind_info(0, &info, sizeof info) == DOTNET_PAL_INVALID_ARGUMENT);
    assert(im->unwind_info(address, &info, sizeof info - 1) == DOTNET_PAL_INVALID_ARGUMENT && info.base == 0);
    long page = sysconf(_SC_PAGESIZE);
    assert(page > 0);
    void *inaccessible = mmap(NULL, (size_t)page * 2, PROT_NONE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    assert(inaccessible != MAP_FAILED);
    assert(im->readable(address, 16) == 0);
    assert(im->readable((uintptr_t)inaccessible, (size_t)page) == DOTNET_PAL_NOT_FOUND);
    assert(im->readable((uintptr_t)inaccessible + (size_t)page + 8, 8) == DOTNET_PAL_NOT_FOUND);
    void *heap = malloc(3 * (size_t)page);
    assert(heap && im->readable((uintptr_t)heap, 3 * (size_t)page) == 0);
    free(heap);
    assert(munmap(inaccessible, (size_t)page * 2) == 0);
    assert(im->readable(0, 8) == DOTNET_PAL_INVALID_ARGUMENT);
    assert(im->readable(address, 0) == DOTNET_PAL_INVALID_ARGUMENT);
    /* The module base is what dladdr reports: the start of the image's first PT_LOAD. */
    Dl_info module;
    assert(dladdr((void*)address, &module) != 0 && module.dli_fbase != NULL);
    uintptr_t base = (uintptr_t)module.dli_fbase;
    uint8_t id[64]; size_t needed = 0;
    memset(id, 0xAA, sizeof id);
    uint32_t status = im->build_id(base, id, sizeof id, &needed);
    /* The test binary is linked with --build-id=sha1 (20 bytes) by scripts/platform.sh. */
    assert(status == 0 && needed == 20);
    for (size_t i = needed; i < sizeof id; ++i) assert(id[i] == 0);
    uint8_t small[4]; needed = 0;
    assert(im->build_id(base, small, sizeof small, &needed) == DOTNET_PAL_BUFFER_TOO_SMALL && needed == 20);
    assert(im->build_id(base, id, sizeof id, NULL) == DOTNET_PAL_INVALID_ARGUMENT);
    assert(im->build_id(address + 1, id, sizeof id, &needed) == DOTNET_PAL_NOT_FOUND); /* not an image base */
    dotnet_pal_image_stats stats;
    assert(im->read_stats(&stats, sizeof stats) == 0 && stats.unwind_ok == 1 && stats.readable_ok >= 2 && stats.build_id_ok == 1 && stats.rejected_or_failed >= 6);
    printf("IMAGE PASS text=%#lx+%zu eh_frame_hdr=%#lx build_id=%zu bytes\n", (unsigned long)found.text_start, found.text_length, (unsigned long)found.eh_frame_hdr, needed);
    return 0;
}

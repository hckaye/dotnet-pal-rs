/* Independent POSIX reference provider for the host-mappings conformance suite. It
 * takes the handles of tests/files_host.c. Where the Linux provider lets the kernel
 * refuse, this one looks first: the open mode of the descriptor decides the access
 * rule, the node's kind what is no file handle, the page size which offset is off
 * the grid. Fault 1 withholds a callback; fault 2 breaks the output contracts so
 * the front end's sanitizing is observable. */
#define _GNU_SOURCE
#include "dotnet_pal.h"
#include <errno.h>
#include <fcntl.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <unistd.h>
int pal_mappings_fault;
int pal_files_host_descriptor(void *file);
static uint32_t map(void *file, uint64_t offset, size_t length, uint32_t access, uint32_t mode, void **address) {
    if (pal_mappings_fault == 2) {
        static int step, target;
        switch (step++) {
        case 0: *address = NULL; return DOTNET_PAL_OK;                       /* success without an address */
        case 1: *address = (void*)(UINTPTR_MAX - 15); return DOTNET_PAL_OK;  /* a range that wraps around the address space */
        case 2: *address = &target; return 99u;                              /* no such status */
        case 3: *address = &target; return DOTNET_PAL_NOT_FOUND;             /* a status this group does not have */
        case 4: *address = &target; return DOTNET_PAL_OUT_OF_MEMORY;         /* outputs written by a failing call */
        default: *address = &target; return DOTNET_PAL_ACCESS_DENIED;
        }
    }
    int fd = pal_files_host_descriptor(file), opened = fcntl(fd, F_GETFL);
    struct stat st;
    if (opened < 0 || fstat(fd, &st) != 0 || S_ISDIR(st.st_mode)) return DOTNET_PAL_INVALID_ARGUMENT; /* no handle of the files group */
    if ((opened & O_ACCMODE) == O_WRONLY) return DOTNET_PAL_ACCESS_DENIED;
    if (mode == DOTNET_PAL_MAP_SHARED && (access & DOTNET_PAL_WRITE) && (opened & O_ACCMODE) != O_RDWR) return DOTNET_PAL_ACCESS_DENIED;
    if (offset % (uint64_t)sysconf(_SC_PAGESIZE)) return DOTNET_PAL_INVALID_ARGUMENT;
    int protection = (access & DOTNET_PAL_READ ? PROT_READ : 0) | (access & DOTNET_PAL_WRITE ? PROT_WRITE : 0) | (access & DOTNET_PAL_EXECUTE ? PROT_EXEC : 0);
    void *mapped = mmap(NULL, length, protection, mode == DOTNET_PAL_MAP_SHARED ? MAP_SHARED : MAP_PRIVATE, fd, (off_t)offset);
    if (mapped != MAP_FAILED) { *address = mapped; return DOTNET_PAL_OK; }
    switch (errno) {
    case EACCES: case EPERM: return DOTNET_PAL_ACCESS_DENIED;
    case ENOMEM: return DOTNET_PAL_OUT_OF_MEMORY;
    case EINVAL: case EOVERFLOW: return DOTNET_PAL_INVALID_ARGUMENT;
    case ENODEV: return DOTNET_PAL_UNSUPPORTED; /* a node or a file system without mappings */
    default: return DOTNET_PAL_OS_ERROR;
    }
}
static uint32_t unmap(void *address, size_t length) {
    if (pal_mappings_fault == 2) { static int step; return step++ ? 99u : DOTNET_PAL_OUT_OF_MEMORY; } /* a status only map has, and no status at all */
    if (munmap(address, length) == 0) return DOTNET_PAL_OK;
    return errno == EINVAL ? DOTNET_PAL_INVALID_ARGUMENT : errno == ENOMEM ? DOTNET_PAL_OUT_OF_MEMORY : DOTNET_PAL_OS_ERROR;
}
static uint32_t sync_mapping(void *address, size_t length) {
    if (pal_mappings_fault == 2) { static int step; return step++ ? DOTNET_PAL_TIMEOUT : DOTNET_PAL_ACCESS_DENIED; } /* a status only map has, and one of the kernel group */
    int rc; do rc = msync(address, length, MS_SYNC); while (rc != 0 && errno == EINTR);
    if (rc == 0) return DOTNET_PAL_OK;
    return errno == EINVAL || errno == ENOMEM ? DOTNET_PAL_INVALID_ARGUMENT : DOTNET_PAL_OS_ERROR; /* ENOMEM: the range is not mapped */
}
static const dotnet_pal_host_mappings table = {
    {DOTNET_PAL_ABI_VERSION, sizeof(dotnet_pal_host_mappings), DOTNET_PAL_CAP_MAPPINGS}, {map, unmap, sync_mapping, NULL},
};
static const dotnet_pal_host_mappings malformed = {
    {DOTNET_PAL_ABI_VERSION, sizeof(dotnet_pal_host_mappings), DOTNET_PAL_CAP_MAPPINGS}, {map, unmap, NULL, NULL},
};
const dotnet_pal_host_mappings *dotnet_pal_host_mappings_v2(void) { return pal_mappings_fault == 1 ? &malformed : &table; }

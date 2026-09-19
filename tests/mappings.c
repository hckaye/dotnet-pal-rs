/* Conformance test of the mappings group on Linux: every mapping is compared with
 * what the kernel lists for it in /proc/self/maps and with the file as the files
 * group and the test's own descriptor read it, every refusal with the kernel's
 * answer to the same request. The handles are those of the files group of the
 * same table. */
#define _GNU_SOURCE
#include "dotnet_pal.h"
#include <assert.h>
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <unistd.h>
#ifdef PAL_HOST_TEST
extern int pal_mappings_fault;
/* A handle no open returned, for a descriptor no open would hand out. tests/files_host.c: the descriptor plus one. */
#define HANDLE(fd) ((void*)(intptr_t)((fd) + 1))
#else
/* src/linux_files.rs: the handle points at the descriptor. */
#define HANDLE(fd) ((void*)&(fd))
#endif
enum { R = DOTNET_PAL_READ, W = DOTNET_PAL_WRITE, X = DOTNET_PAL_EXECUTE, SHARED = DOTNET_PAL_MAP_SHARED, PRIVATE = DOTNET_PAL_MAP_PRIVATE };
#define INVALID DOTNET_PAL_INVALID_ARGUMENT
#define DENIED DOTNET_PAL_ACCESS_DENIED
#define S(text) (const uint8_t*)(text), strlen(text)
static const dotnet_pal_mappings_ops *m;
static const dotnet_pal_files_ops *f;
static size_t page;
/* What the counters have to say at the end: every call of the group goes through these three. */
static dotnet_pal_mappings_stats expected;
static uint32_t map(void *file, uint64_t offset, size_t length, uint32_t access, uint32_t mode, void **address) {
    uint32_t status = m->map(file, offset, length, access, mode, address);
    ++*(status == 0 ? &expected.map_ok : &expected.rejected_or_failed);
    assert((status == 0) == (address == NULL || *address != NULL)); /* an address with OK, and with nothing else */
    return status;
}
static uint32_t unmap(void *address, size_t length) { uint32_t status = m->unmap(address, length); ++*(status == 0 ? &expected.unmap_ok : &expected.rejected_or_failed); return status; }
static uint32_t synchronize(void *address, size_t length) { uint32_t status = m->sync(address, length); ++*(status == 0 ? &expected.sync_ok : &expected.rejected_or_failed); return status; }
static uint8_t *mapped(void *file, uint64_t offset, size_t length, uint32_t access, uint32_t mode) {
    void *address = (void*)1;
    assert(map(file, offset, length, access, mode, &address) == 0 && address && (uintptr_t)address % page == 0);
    return address;
}
static void refused(void *file, uint64_t offset, size_t length, uint32_t access, uint32_t mode, uint32_t status) {
    void *address = (void*)1;
    assert(map(file, offset, length, access, mode, &address) == status && address == NULL);
}
/* The kernel's own answer to a request: 0 when it grants the mapping, its errno when it refuses. */
static int kernel(int fd, uint64_t offset, size_t length, int protection, int flags) {
    void *address = mmap(NULL, length, protection, flags, fd, (off_t)offset);
    if (address == MAP_FAILED) return errno;
    assert(munmap(address, length) == 0); return 0;
}
/* The kernel lists [address, address + length) as one mapping of the data file from `offset` on with these permissions ("rw-s"); with no
 * permissions asked for, it lists no mapping of the file there. The file is known by its path: behind an overlay the listed inode may be
 * the lower file's. Neighbours that continue one another are listed as one line. */
static char data[64];
static void listed(const void *address, size_t length, uint64_t offset, const char *permissions) {
    char line[512]; int found = 0;
    uintptr_t from = (uintptr_t)address, to = from + (length + page - 1) / page * page;
    FILE *maps = fopen("/proc/self/maps", "r"); assert(maps);
    while (fgets(line, sizeof line, maps)) {
        unsigned long start, end; unsigned long long at; char mode[8]; int path = 0;
        if (sscanf(line, "%lx-%lx %7s %llx %*x:%*x %*u %n", &start, &end, mode, &at, &path) != 4 || !path) continue;
        line[strcspn(line, "\n")] = 0;
        if (end <= from || start >= to || strcmp(line + path, data) != 0) continue;
        assert(permissions && start <= from && to <= end && strcmp(mode, permissions) == 0 && at + (from - start) == offset);
        found = 1;
    }
    fclose(maps);
    assert(found == (permissions != NULL));
}
static int zero(const uint8_t *bytes, size_t size) { while (size--) if (*bytes++) return 0; return 1; }
int main(int argc, char **argv) {
#ifdef PAL_HOST_TEST
    pal_mappings_fault = argc > 1 ? atoi(argv[1]) : 0;
#else
    (void)argc; (void)argv;
#endif
    const dotnet_pal_api *api = dotnet_pal_get_api(2);
#ifdef PAL_HOST_TEST
    if (pal_mappings_fault == 1) { assert(!api); puts("MAPPINGS malformed host rejected"); return 0; }
#endif
    assert(api && api->header.struct_size >= DOTNET_PAL_MAPPINGS_API_SIZE);
    assert((api->header.capabilities & DOTNET_PAL_CAP_MAPPINGS) && (api->header.capabilities & DOTNET_PAL_CAP_FILES));
    m = &api->mappings; f = &api->files;
    assert(m->map && m->unmap && m->sync && m->read_stats);
    page = (size_t)sysconf(_SC_PAGESIZE);
    dotnet_pal_mappings_stats before, after;
    void *address = (void*)1;
#ifdef PAL_HOST_TEST
    if (pal_mappings_fault == 2) {
        int handle = 0; /* any non-null handle: this provider answers before it looks at one */
        /* Success without an address or with a range that ends behind the address space, and statuses the call does not have, are OS_ERROR;
         * a failure keeps its status where the call has it, and never the address the provider wrote. */
        const uint32_t answers[] = {DOTNET_PAL_OS_ERROR, DOTNET_PAL_OS_ERROR, DOTNET_PAL_OS_ERROR, DOTNET_PAL_OS_ERROR, DOTNET_PAL_OUT_OF_MEMORY, DENIED};
        for (size_t i = 0; i < sizeof answers / sizeof *answers; ++i) { address = (void*)1; assert(m->map(&handle, 0, page, R, SHARED, &address) == answers[i] && address == NULL); }
        for (int i = 0; i < 2; ++i) assert(m->unmap(&handle, page) == DOTNET_PAL_OS_ERROR && m->sync(&handle, page) == DOTNET_PAL_OS_ERROR);
        assert(m->read_stats(&after, sizeof after) == 0 && after.rejected_or_failed == 10 && after.map_ok + after.unmap_ok + after.sync_ok == 0);
        puts("MAPPINGS host errors sanitized"); return 0;
    }
#endif
    /* Three pages and five bytes, every byte telling its position. */
    char root[] = "/tmp/pal-mappings-XXXXXX";
    assert(mkdtemp(root) && chdir(root) == 0 && (size_t)snprintf(data, sizeof data, "%s/data", root) < sizeof data);
    const size_t size = 3 * page + 5, whole = 4 * page;
    uint8_t *content = malloc(size + 16), *read = malloc(size + 16);
    assert(content && read);
    for (size_t i = 0; i < size; ++i) content[i] = (uint8_t)(i % 251);
    int own = open("data", O_RDWR | O_CREAT | O_EXCL | O_CLOEXEC, 0600), read_only = open("data", O_RDONLY | O_CLOEXEC), write_only = open("data", O_WRONLY | O_CLOEXEC);
    assert(own >= 0 && read_only >= 0 && write_only >= 0 && write(own, content, size) == (ssize_t)size);
    void *file = NULL, *reader = NULL, *writer = NULL, *node = NULL; size_t done = 7;
    assert(f->open(S("data"), R | W, 0, &file) == 0 && file);

    /* Two shared mappings up to the end of the page that holds the last byte: the file's bytes, then zeros. */
    uint8_t *first = mapped(file, 0, whole, R | W, SHARED), *second = mapped(file, 0, whole, R, SHARED);
    assert(first != second);
    listed(first, whole, 0, "rw-s"); listed(second, whole, 0, "r--s");
    assert(memcmp(first, content, size) == 0 && memcmp(second, content, size) == 0 && zero(first + size, whole - size) && zero(second + size, whole - size));
    /* A write through a shared mapping is in the file, as the files group and the kernel read it, and in the other shared mapping. */
    memcpy(first + 10, "HELLO", 5); memcpy(content + 10, "HELLO", 5);
    assert(f->read_at(file, 0, read, size + 16, &done) == 0 && done == size && memcmp(read, content, size) == 0);
    assert(pread(own, read, size + 16, 0) == (ssize_t)size && memcmp(read, content, size) == 0 && memcmp(second, content, size) == 0);
    /* A write through the handle is in both. */
    assert(f->write_at(file, page + 3, (const uint8_t*)"through the handle", 18, &done) == 0 && done == 18);
    memcpy(content + page + 3, "through the handle", 18);
    assert(memcmp(first, content, size) == 0 && memcmp(second, content, size) == 0);
    /* A private mapping starts as the file and keeps what is written to it to itself. Its length is no multiple of the page size. */
    uint8_t *third = mapped(file, 0, size, R | W, PRIVATE);
    listed(third, size, 0, "rw-p");
    assert(memcmp(third, content, size) == 0);
    memcpy(third + 20, "PRIVATE", 7);
    assert(f->read_at(file, 0, read, size + 16, &done) == 0 && done == size && memcmp(read, content, size) == 0);
    assert(memcmp(first, content, size) == 0 && memcmp(second, content, size) == 0 && memcmp(third + 20, "PRIVATE", 7) == 0);
    /* sync: of the writable shared mapping, of the one that only reads, and of the private one, which has nothing to write. */
    assert(synchronize(first, whole) == 0 && synchronize(second, whole) == 0 && synchronize(third, size) == 0);
    assert(pread(own, read, size + 16, 0) == (ssize_t)size && memcmp(read, content, size) == 0);
    /* What is written behind the last byte does not become part of the file. */
    struct stat st;
    first[size + 1] = 'x';
    assert(synchronize(first, whole) == 0 && fstat(own, &st) == 0 && (size_t)st.st_size == size && pread(own, read, 16, (off_t)size) == 0);

    /* The mappings outlive the handle. */
    assert(f->close(file) == 0);
    first[0] = '#'; content[0] = '#';
    assert(second[0] == '#' && synchronize(first, whole) == 0 && pread(own, read, size, 0) == (ssize_t)size && memcmp(read, content, size) == 0);
    listed(first, whole, 0, "rw-s");

    /* A part in the middle of the file, and its last page alone, through a handle that only reads. */
    assert(f->open(S("data"), R, 0, &reader) == 0 && reader);
    uint8_t *middle = mapped(reader, page, page + 7, R, SHARED), *last = mapped(reader, 3 * page, page, R, SHARED);
    listed(middle, page + 7, page, "r--s"); listed(last, page, 3 * page, "r--s");
    assert(memcmp(middle, content + page, page + 7) == 0 && memcmp(last, content + 3 * page, 5) == 0 && zero(last + 5, page - 5));
    first[page + 100] = '!'; content[page + 100] = '!';
    assert(middle[100] == '!');

    /* The access rule, which is the kernel's: shared and writable needs write access, private and writable does not. */
    assert(kernel(read_only, 0, page, PROT_READ | PROT_WRITE, MAP_SHARED) == EACCES && kernel(read_only, 0, page, PROT_WRITE, MAP_SHARED) == EACCES);
    refused(reader, 0, page, R | W, SHARED, DENIED); refused(reader, 0, page, W, SHARED, DENIED);
    assert(kernel(read_only, 2 * page, page, PROT_READ | PROT_WRITE, MAP_PRIVATE) == 0);
    uint8_t *copy = mapped(reader, 2 * page, page, R | W, PRIVATE);
    listed(copy, page, 2 * page, "rw-p");
    copy[0] = '?';
    assert(first[2 * page] == content[2 * page] && pread(own, read, 1, (off_t)(2 * page)) == 1 && read[0] == content[2 * page]);
    /* A handle that only writes maps nothing, whatever the mapping would be used for. */
    assert(f->open(S("data"), W, 0, &writer) == 0 && writer);
    for (uint32_t mode = SHARED; mode <= PRIVATE; ++mode) for (uint32_t access = R; access <= (R | W); ++access) {
        int protection = (access & R ? PROT_READ : 0) | (access & W ? PROT_WRITE : 0);
        assert(kernel(write_only, 0, page, protection, mode == SHARED ? MAP_SHARED : MAP_PRIVATE) == EACCES);
        refused(writer, 0, page, access, mode, DENIED);
    }
    assert(f->close(writer) == 0);

    /* An offset off the page grid. */
    const uint64_t crooked[] = {1, page - 1, page + 1, 2 * page + 512};
    for (size_t i = 0; i < sizeof crooked / sizeof *crooked; ++i) {
        assert(kernel(read_only, crooked[i], page, PROT_READ, MAP_SHARED) == EINVAL);
        refused(reader, crooked[i], page, R, SHARED, INVALID);
    }
    /* A range behind the end of the file is the kernel's to grant, and it does: what is touched there is a bus error, which nothing here does. */
    assert(kernel(read_only, 8 * page, page, PROT_READ, MAP_SHARED) == 0);
    uint8_t *beyond = mapped(reader, 8 * page, page, R, SHARED);
    listed(beyond, page, 8 * page, "r--s");
    /* An executable mapping: granted unless the volume is mounted noexec. */
    int executable = kernel(read_only, 0, page, PROT_READ | PROT_EXEC, MAP_PRIVATE);
    assert(executable == 0 || executable == EPERM);
    uint8_t *code = NULL;
    if (executable == 0) { code = mapped(reader, 0, page, R | X, PRIVATE); listed(code, page, 0, "r-xp"); assert(memcmp(code, content, page) == 0); }
    else refused(reader, 0, page, R | X, PRIVATE, DENIED);

    /* Nodes without pages are UNSUPPORTED; the zero device has them. */
    int null = open("/dev/null", O_RDWR | O_CLOEXEC), zeros = open("/dev/zero", O_RDWR | O_CLOEXEC);
    assert(null >= 0 && zeros >= 0 && kernel(null, 0, page, PROT_READ, MAP_SHARED) == ENODEV && kernel(null, 0, page, PROT_READ, MAP_PRIVATE) == ENODEV);
    assert(f->open(S("/dev/null"), R | W, 0, &node) == 0 && node);
    refused(node, 0, page, R, SHARED, DOTNET_PAL_UNSUPPORTED); refused(node, 0, page, R | W, PRIVATE, DOTNET_PAL_UNSUPPORTED);
    assert(f->close(node) == 0 && kernel(zeros, 0, page, PROT_READ | PROT_WRITE, MAP_PRIVATE) == 0);
    assert(f->open(S("/dev/zero"), R | W, 0, &node) == 0 && node);
    uint8_t *blank = mapped(node, 0, page, R | W, PRIVATE);
    assert(zero(blank, page) && f->close(node) == 0);
    /* What is no handle of the files group: a directory, which the kernel calls ENODEV, and a descriptor that is not open. */
    int directory = open(".", O_RDONLY | O_DIRECTORY | O_CLOEXEC), closed = dup(own);
    assert(directory >= 0 && closed >= 0 && close(closed) == 0);
    assert(kernel(directory, 0, page, PROT_READ, MAP_SHARED) == ENODEV && kernel(closed, 0, page, PROT_READ, MAP_SHARED) == EBADF);
    refused(HANDLE(directory), 0, page, R, SHARED, INVALID); refused(HANDLE(directory), 0, page, R, PRIVATE, INVALID);
    refused(HANDLE(closed), 0, page, R, SHARED, INVALID);

    /* unmap: the kernel lists the range no more, and the others are as they were. */
    assert(f->close(reader) == 0);
    assert(unmap(second, whole) == 0 && unmap(third, size) == 0);
    listed(second, whole, 0, NULL); listed(third, size, 0, NULL); listed(first, whole, 0, "rw-s");
    assert(memcmp(first, content, size) == 0 && memcmp(middle, content + page, page) == 0);
    assert(unmap(first, whole) == 0 && unmap(middle, page + 7) == 0 && unmap(last, page) == 0 && unmap(copy, page) == 0 && unmap(beyond, page) == 0 && unmap(blank, page) == 0);
    if (code) assert(unmap(code, page) == 0);
    listed(first, whole, 0, NULL); listed(middle, page + 7, page, NULL); listed(last, page, 3 * page, NULL);
    assert(pread(own, read, size + 16, 0) == (ssize_t)size && memcmp(read, content, size) == 0);

    /* Argument validation: refused before a provider runs, with one count each and nothing else. */
    assert(f->open(S("data"), R | W, 0, &file) == 0 && file);
    assert(m->read_stats(&before, sizeof before) == 0);
    assert(m->map(file, 0, page, R, SHARED, NULL) == INVALID && m->map(file, 0, page, R, SHARED, (void**)((char*)&address + 1)) == INVALID && address == (void*)1);
    assert(m->map(NULL, 0, page, R, SHARED, &address) == INVALID && address == NULL);
    const size_t lengths[] = {0, (size_t)PTRDIFF_MAX + 1, SIZE_MAX};
    for (size_t i = 0; i < 3; ++i) { address = (void*)1; assert(m->map(file, 0, lengths[i], R, SHARED, &address) == INVALID && address == NULL); }
    /* The last byte of a mapping has an offset the files group can name. */
    const uint64_t offsets[] = {(uint64_t)INT64_MAX - page + 1, (uint64_t)INT64_MAX + 1, UINT64_MAX - page + 1, UINT64_MAX};
    for (size_t i = 0; i < 4; ++i) { address = (void*)1; assert(m->map(file, offsets[i], page, R, SHARED, &address) == INVALID && address == NULL); }
    const uint32_t accesses[] = {0, 8, R | 8, UINT32_MAX}, modes[] = {0, SHARED | PRIVATE, 4, UINT32_MAX};
    for (size_t i = 0; i < 4; ++i) { address = (void*)1; assert(m->map(file, 0, page, accesses[i], SHARED, &address) == INVALID && address == NULL); }
    for (size_t i = 0; i < 4; ++i) { address = (void*)1; assert(m->map(file, 0, page, R, modes[i], &address) == INVALID && address == NULL); }
    assert(m->unmap(NULL, page) == INVALID && m->unmap(&address, 0) == INVALID && m->unmap(&address, (size_t)PTRDIFF_MAX + 1) == INVALID && m->unmap((void*)(UINTPTR_MAX - page + 2), page) == INVALID);
    assert(m->sync(NULL, page) == INVALID && m->sync(&address, 0) == INVALID && m->sync(&address, (size_t)PTRDIFF_MAX + 1) == INVALID && m->sync((void*)(UINTPTR_MAX - page + 2), page) == INVALID);
    assert(m->read_stats(NULL, sizeof after) == INVALID && m->read_stats(&after, sizeof after - 1) == INVALID);
    assert(m->read_stats(&after, sizeof after) == 0 && after.rejected_or_failed == before.rejected_or_failed + 26);
    assert(after.map_ok == before.map_ok && after.unmap_ok == before.unmap_ok && after.sync_ok == before.sync_ok);
    expected.rejected_or_failed += 26;

    /* Counters: one per call that did what it was asked, one for everything refused. */
    uint8_t *counted = mapped(file, 0, page, R, SHARED);
    assert(synchronize(counted, page) == 0 && unmap(counted, page) == 0);
    refused(file, 1, page, R, SHARED, INVALID);
    assert(m->read_stats(&after, sizeof after) == 0);
    assert(after.map_ok == before.map_ok + 1 && after.sync_ok == before.sync_ok + 1 && after.unmap_ok == before.unmap_ok + 1 && after.rejected_or_failed == before.rejected_or_failed + 27);
    assert(after.map_ok == expected.map_ok && after.unmap_ok == expected.unmap_ok && after.sync_ok == expected.sync_ok && after.rejected_or_failed == expected.rejected_or_failed);
    assert(after.map_ok == after.unmap_ok && after.map_ok == (code ? 10u : 9u) && after.sync_ok == 6 && after.rejected_or_failed == (code ? 44u : 45u));

    assert(f->close(file) == 0 && close(own) == 0 && close(read_only) == 0 && close(write_only) == 0 && close(null) == 0 && close(zeros) == 0 && close(directory) == 0);
    assert(unlink("data") == 0 && chdir("/") == 0 && rmdir(root) == 0);
    free(content); free(read);
    printf("MAPPINGS PASS page=%zu file=%zu maps=%llu unmaps=%llu syncs=%llu refused=%llu executable=%s\n", page, size, (unsigned long long)after.map_ok,
        (unsigned long long)after.unmap_ok, (unsigned long long)after.sync_ok, (unsigned long long)after.rejected_or_failed, code ? "granted" : "denied");
    return 0;
}

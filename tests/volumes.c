/* Conformance test of the volumes group on Linux: the enumeration is compared with
 * what getmntent reads from the same table, the space of a volume with statvfs,
 * and its format with the type text of the mount that df would name for the path:
 * the longest mount point that leads to it. The providers match devices instead. */
#define _GNU_SOURCE
#include "dotnet_pal.h"
#include <assert.h>
#include <errno.h>
#include <fcntl.h>
#include <grp.h>
#include <limits.h>
#include <mntent.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <sys/statvfs.h>
#include <sys/wait.h>
#include <unistd.h>
#ifdef PAL_HOST_TEST
extern int pal_volumes_fault;
#endif
#define INVALID DOTNET_PAL_INVALID_ARGUMENT
#define SMALL DOTNET_PAL_BUFFER_TOO_SMALL
#define MISSING DOTNET_PAL_NOT_FOUND
#define CAPACITY (DOTNET_PAL_MAX_NAME + 1)
#define S(text) (const uint8_t*)(text), strlen(text)
static const dotnet_pal_volumes_ops *v;
static uint8_t out[CAPACITY + 1];
/* What the counters have to say at the end: every call of the group goes through these two. */
static dotnet_pal_volumes_stats expected;
static uint32_t entry(size_t index, uint8_t *buffer, size_t capacity, size_t *needed) {
    uint32_t status = v->entry(index, buffer, capacity, needed);
    ++*(status == 0 ? &expected.entry_ok : &expected.rejected_or_failed); return status;
}
static uint32_t status_of(const char *path, dotnet_pal_volume_status *s) {
    memset(s, 0x55, sizeof *s);
    uint32_t status = v->status(S(path), s, sizeof *s);
    ++*(status == 0 ? &expected.status_ok : &expected.rejected_or_failed); return status;
}
static int zero(const uint8_t *bytes, size_t size) { while (size--) if (*bytes++) return 0; return 1; }
/* The table as the C library reads it. */
struct mount { char *point, *type; };
static struct mount *mounts; static size_t count;
static void read_table(void) {
    for (size_t i = 0; i < count; ++i) { free(mounts[i].point); free(mounts[i].type); }
    count = 0;
    FILE *table = setmntent("/proc/self/mounts", "r"); assert(table);
    for (struct mntent *m; (m = getmntent(table));) {
        mounts = realloc(mounts, (count + 1) * sizeof *mounts); assert(mounts);
        mounts[count].point = strdup(m->mnt_dir); mounts[count].type = strdup(m->mnt_type);
        assert(mounts[count].point && mounts[count].type); ++count;
    }
    endmntent(table);
}
/* The type text of the longest mount point that leads to a resolved path, the last of them where one covers another. */
static const char *holder(const char *path) {
    const char *type = NULL; size_t longest = 0;
    for (size_t i = 0; i < count; ++i) {
        size_t length = strlen(mounts[i].point);
        int leads = strncmp(path, mounts[i].point, length) == 0 && (length == 1 || path[length] == 0 || path[length] == '/');
        if (leads && length >= longest) { longest = length; type = mounts[i].type; }
    }
    assert(type); return type;
}
/* Entry `index` is `text`: whole with its NUL where it fits, cleared behind it and never past the capacity; the length alone, and a cleared buffer, where it does not. */
static void delivers(size_t index, const char *text) {
    size_t length = strlen(text) + 1, needed = 7;
    assert(length >= 2 && length <= CAPACITY);
    memset(out, 0xAA, sizeof out);
    assert(entry(index, out, CAPACITY, &needed) == 0 && needed == length && memcmp(out, text, length) == 0 && zero(out + length, CAPACITY - length) && out[CAPACITY] == 0xAA);
    memset(out, 0xAA, sizeof out); needed = 7;
    assert(entry(index, out, length, &needed) == 0 && needed == length && memcmp(out, text, length) == 0 && out[length] == 0xAA);
    memset(out, 0xAA, sizeof out); needed = 7;
    assert(entry(index, out, length - 1, &needed) == SMALL && needed == length && zero(out, length - 1) && out[length - 1] == 0xAA);
    needed = 7;
    assert(entry(index, NULL, 0, &needed) == SMALL && needed == length);
}
static void refuses(size_t index, uint32_t status) {
    size_t needed = 7;
    memset(out, 0xAA, sizeof out);
    assert(entry(index, out, CAPACITY, &needed) == status && needed == 0 && zero(out, CAPACITY) && out[CAPACITY] == 0xAA);
}
/* The status of a path is what statvfs measures and the type text of the mount that holds it. Free space moves under a running
 * system, so the numbers are compared once two measurements around the call agree. Returns the capacity. */
static uint64_t measured(const char *path, const char *type) {
    dotnet_pal_volume_status s; struct statvfs earlier, later; char resolved[PATH_MAX];
    for (int attempt = 0;; ++attempt) {
        assert(statvfs(path, &earlier) == 0 && status_of(path, &s) == 0 && statvfs(path, &later) == 0);
        if (earlier.f_blocks == later.f_blocks && earlier.f_bfree == later.f_bfree && earlier.f_bavail == later.f_bavail) break;
        assert(attempt < 50);
    }
    uint64_t unit = earlier.f_frsize;
    assert(s.total_bytes == earlier.f_blocks * unit && s.free_bytes == earlier.f_bfree * unit && s.available_bytes == earlier.f_bavail * unit);
    assert(s.available_bytes <= s.free_bytes && s.free_bytes <= s.total_bytes);
    if (!type) { assert(realpath(path, resolved)); type = holder(resolved); }
    size_t length = strlen(type);
    assert(length > 0 && length < sizeof s.format && memcmp(s.format, type, length) == 0 && zero(s.format + length, sizeof s.format - length));
    return s.total_bytes;
}
static void missing(const char *path, int code, uint32_t status) {
    dotnet_pal_volume_status s; struct statvfs k;
    errno = 0;
    assert(statvfs(path, &k) == -1 && errno == code && status_of(path, &s) == status && zero((const uint8_t*)&s, sizeof s));
}
/* ACCESS_DENIED needs a caller the permission bits apply to: a child that gives its privileges up. 77 when the kernel lets it through. */
static int denied(const char *path) {
    struct statvfs k;
    if (setgroups(0, NULL) != 0 || setgid(54321) != 0 || setuid(54321) != 0) return 77;
    if (statvfs(path, &k) == 0 || errno != EACCES) return 77;
    missing(path, EACCES, DOTNET_PAL_ACCESS_DENIED);
    return 0;
}
int main(int argc, char **argv) {
#ifdef PAL_HOST_TEST
    pal_volumes_fault = argc > 1 ? atoi(argv[1]) : 0;
#else
    (void)argc; (void)argv;
#endif
    const dotnet_pal_api *api = dotnet_pal_get_api(2);
#ifdef PAL_HOST_TEST
    if (pal_volumes_fault == 1) { assert(!api); puts("VOLUMES malformed host rejected"); return 0; }
#endif
    assert(api && api->header.struct_size >= DOTNET_PAL_VOLUMES_API_SIZE);
    assert(api->header.capabilities & DOTNET_PAL_CAP_VOLUMES);
    v = &api->volumes;
    assert(v->entry && v->status && v->read_stats);
    dotnet_pal_volumes_stats before, after; dotnet_pal_volume_status s; size_t needed = 7;
#ifdef PAL_HOST_TEST
    if (pal_volumes_fault == 2) {
        /* A text without its NUL, with one inside, empty, without a length, behind a status the call does not have, longer than any path or
         * "too small" for a buffer it fits is refused and cleared; the end is the enumeration's to report, without the outputs of that call. */
        for (int i = 0; i < 8; ++i) { needed = 7; memset(out, 0xAA, sizeof out); assert(v->entry(0, out, CAPACITY, &needed) == DOTNET_PAL_OS_ERROR && needed == 0 && zero(out, CAPACITY)); }
        needed = 7; memset(out, 0xAA, sizeof out);
        assert(v->entry(0, out, CAPACITY, &needed) == MISSING && needed == 0 && zero(out, CAPACITY));
        /* More free than there is, more usable than free, a format without its end and statuses the call does not have are no answer. */
        for (int i = 0; i < 5; ++i) { memset(&s, 0x55, sizeof s); assert(v->status(S("/"), &s, sizeof s) == DOTNET_PAL_OS_ERROR && zero((const uint8_t*)&s, sizeof s)); }
        memset(&s, 0x55, sizeof s);
        assert(v->status(S("/"), &s, sizeof s) == MISSING && zero((const uint8_t*)&s, sizeof s));
        assert(v->read_stats(&after, sizeof after) == 0 && after.rejected_or_failed == 15 && after.entry_ok + after.status_ok == 0);
        puts("VOLUMES host errors sanitized"); return 0;
    }
#endif
    char root[] = "/tmp/pal-volumes-XXXXXX", path[PATH_MAX];
    assert(mkdtemp(root));
    /* A volume of this test's own where the process may mount: a megabyte of tmpfs on a path with every byte the table escapes. */
    snprintf(path, sizeof path, "%s/a b\tc\\d\ne", root);
    assert(mkdir(path, 0700) == 0);
    int escaped = mount("pal-volumes", path, "tmpfs", 0, "size=1m") == 0;
    assert(escaped || errno == EPERM);

    /* Enumeration: the mount points of the table in its order, NOT_FOUND from the count on. Each is a path that is there and on a volume. */
    read_table();
    int has_root = 0, own = 0;
    for (size_t i = 0; i < count; ++i) {
        struct stat node; struct statvfs space;
        delivers(i, mounts[i].point);
        assert(mounts[i].point[0] == '/' && lstat(mounts[i].point, &node) == 0 && statvfs(mounts[i].point, &space) == 0);
        if (strcmp(mounts[i].point, "/") == 0) has_root = 1;
        if (strcmp(mounts[i].point, path) == 0) ++own;
    }
    assert(count > 0 && has_root && own == escaped);
    refuses(count, MISSING); refuses(count + 1, MISSING); refuses(SIZE_MAX, MISSING);

    /* status: the root, the scratch directory, procfs, a file, and every mount point, among them those that are files and those another covers. */
    uint64_t total = measured("/", NULL);
    measured(root, NULL); measured("/proc", "proc"); measured("/proc/self/status", "proc"); measured("/etc/passwd", NULL); measured(".", NULL);
    for (size_t i = 0; i < count; ++i) measured(mounts[i].point, NULL);
    if (escaped) assert(measured(path, "tmpfs") == 1024 * 1024);
    /* A path that names nothing, directly or through a file. */
    missing("/no/such/pal/volume", ENOENT, MISSING); missing("/etc/passwd/x", ENOTDIR, MISSING);
    int refused = -1;
    if (geteuid() == 0) {
        snprintf(path, sizeof path, "%s/locked", root);
        assert(mkdir(path, 0700) == 0);
        strcat(path, "/inside");
        pid_t child = fork(); assert(child >= 0);
        if (child == 0) _exit(denied(path));
        int result = 0;
        assert(waitpid(child, &result, 0) == child && WIFEXITED(result) && (WEXITSTATUS(result) == 0 || WEXITSTATUS(result) == 77));
        refused = WEXITSTATUS(result) == 0;
    }

    /* Argument validation: refused before a provider runs, with one count each and nothing else. */
    assert(v->read_stats(&before, sizeof before) == 0);
    assert(v->entry(0, out, CAPACITY, NULL) == INVALID && v->entry(0, out, CAPACITY, (size_t*)((char*)&needed + 1)) == INVALID && v->entry(0, NULL, 8, &needed) == INVALID);
    assert(v->entry(0, out, (size_t)PTRDIFF_MAX + 1, &needed) == INVALID && v->entry(0, (uint8_t*)(UINTPTR_MAX - 3), 8, &needed) == INVALID && needed == 7);
    memset(&s, 0x55, sizeof s);
    assert(v->status(S("/"), NULL, sizeof s) == INVALID && v->status(S("/"), (void*)((char*)&s + 1), sizeof s) == INVALID && v->status(S("/"), &s, sizeof s - 1) == INVALID && s.total_bytes == UINT64_C(0x5555555555555555));
    /* A path is 1 to MAX_NAME bytes without a NUL; the output is cleared once it is known to be one. */
    static uint8_t longest[DOTNET_PAL_MAX_NAME + 1];
    memset(longest, 'a', sizeof longest);
    const struct { const uint8_t *path; size_t length; } paths[] = {{NULL, 1}, {(const uint8_t*)"/", 0}, {longest, sizeof longest}, {(const uint8_t*)"/a\0b", 4}, {(const uint8_t*)(UINTPTR_MAX - 1), 8}};
    for (size_t i = 0; i < 5; ++i) { memset(&s, 0x55, sizeof s); assert(v->status(paths[i].path, paths[i].length, &s, sizeof s) == INVALID && zero((const uint8_t*)&s, sizeof s)); }
    /* The longest path the boundary takes reaches the provider, where the kernel has the word. */
    memset(&s, 0x55, sizeof s);
    assert(v->status(longest, DOTNET_PAL_MAX_NAME, &s, sizeof s) != INVALID && zero((const uint8_t*)&s, sizeof s));
    assert(v->read_stats(NULL, sizeof after) == INVALID && v->read_stats(&after, sizeof after - 1) == INVALID);
    assert(v->read_stats(&after, sizeof after) == 0 && after.rejected_or_failed == before.rejected_or_failed + 14 && after.entry_ok == before.entry_ok && after.status_ok == before.status_ok);
    expected.rejected_or_failed += 14;

    /* Counters: one per answered question of each kind, one for everything refused. */
    assert(entry(0, out, CAPACITY, &needed) == 0 && status_of("/", &s) == 0 && entry(count, out, CAPACITY, &needed) == MISSING && entry(0, out, 1, &needed) == SMALL);
    assert(status_of("/no/such/pal/volume", &s) == MISSING);
    assert(v->read_stats(&after, sizeof after) == 0);
    assert(after.entry_ok == before.entry_ok + 1 && after.status_ok == before.status_ok + 1 && after.rejected_or_failed == before.rejected_or_failed + 17);
    assert(after.entry_ok == expected.entry_ok && after.status_ok == expected.status_ok && after.rejected_or_failed == expected.rejected_or_failed);
    assert(after.entry_ok == 2 * count + 1 && after.rejected_or_failed == 2 * count + 3 + 2 + 17);

    if (escaped) { snprintf(path, sizeof path, "%s/a b\tc\\d\ne", root); assert(umount(path) == 0); }
    snprintf(path, sizeof path, "%s/a b\tc\\d\ne", root); assert(rmdir(path) == 0);
    if (geteuid() == 0) { snprintf(path, sizeof path, "%s/locked", root); assert(rmdir(path) == 0); }
    assert(rmdir(root) == 0);
    printf("VOLUMES PASS mounts=%zu root=%s scratch=%s total_mib=%llu statuses=%llu refused=%llu escaped_mount=%s access_denied=%s\n", count, holder("/"), holder(root),
        (unsigned long long)(total >> 20), (unsigned long long)after.status_ok, (unsigned long long)after.rejected_or_failed, escaped ? "checked" : "not permitted", refused == 1 ? "checked" : "not checked");
    return 0;
}

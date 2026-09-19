/* Conformance test of the files group on Linux: every callback works on a scratch
 * directory and is checked against what the kernel reports for the same nodes,
 * locks against what the kernel grants a descriptor of its own. */
#define _GNU_SOURCE
#include "dotnet_pal.h"
#include <assert.h>
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <ftw.h>
#include <grp.h>
#include <limits.h>
#include <pthread.h>
#include <sched.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/file.h>
#include <sys/resource.h>
#include <sys/stat.h>
#include <time.h>
#include <sys/wait.h>
#include <unistd.h>
#ifdef PAL_HOST_TEST
extern int pal_files_fault;
#endif
enum { R = DOTNET_PAL_FILE_READ, W = DOTNET_PAL_FILE_WRITE, C = DOTNET_PAL_FILE_CREATE, X = DOTNET_PAL_FILE_EXCLUSIVE, T = DOTNET_PAL_FILE_TRUNCATE };
enum { SH = DOTNET_PAL_LOCK_SHARED, EX = DOTNET_PAL_LOCK_EXCLUSIVE, UN = DOTNET_PAL_LOCK_UNLOCK };
#define INVALID DOTNET_PAL_INVALID_ARGUMENT
#define KEEP DOTNET_PAL_TIME_KEEP
#define NAME DOTNET_PAL_MAX_ENTRY_NAME
/* A path as the byte borrow the boundary takes: no terminator. */
#define S(text) (const uint8_t*)(text), strlen(text)
/* The kernel refuses a call with `code`, and the boundary names the same condition for the same call. */
#define REFUSED(kernel, code, boundary, status) do { errno = 0; assert((kernel) == -1 && errno == (code)); assert((boundary) == (status)); } while (0)
static const dotnet_pal_files_ops *f;
static int zero(const uint8_t *bytes, size_t size) { while (size--) if (*bytes++) return 0; return 1; }
static uint64_t ns(struct timespec time) { return (uint64_t)time.tv_sec * UINT64_C(1000000000) + (uint64_t)time.tv_nsec; }
static uint32_t node(mode_t mode) {
    return S_ISREG(mode) ? DOTNET_PAL_NODE_FILE : S_ISDIR(mode) ? DOTNET_PAL_NODE_DIRECTORY : S_ISLNK(mode) ? DOTNET_PAL_NODE_SYMLINK : DOTNET_PAL_NODE_OTHER;
}
/* A status from the boundary is the kernel's description of the node; the birth time is what statx says, 0 when it has none. */
static void same(const dotnet_pal_file_status *s, const struct stat *k, const char *path, int flags) {
    assert(s->kind == node(k->st_mode) && s->mode == (k->st_mode & 07777u) && s->size == (uint64_t)k->st_size);
    assert(s->modified_ns == ns(k->st_mtim) && s->accessed_ns == ns(k->st_atim) && s->changed_ns == ns(k->st_ctim));
    assert(s->identity == k->st_ino && s->device == k->st_dev);
    struct statx x;
    uint64_t created = statx(AT_FDCWD, path, flags, STATX_BTIME, &x) == 0 && (x.stx_mask & STATX_BTIME) && x.stx_btime.tv_sec >= 0
        ? (uint64_t)x.stx_btime.tv_sec * UINT64_C(1000000000) + x.stx_btime.tv_nsec : 0;
    assert(s->created_ns == created);
}
/* path_status agrees with stat (follow) or lstat about a path of the expected kind. */
static void described(const char *path, uint32_t follow, uint32_t kind) {
    dotnet_pal_file_status s; struct stat k;
    assert(f->path_status(S(path), follow, &s, sizeof s) == 0 && (follow ? stat(path, &k) : lstat(path, &k)) == 0);
    same(&s, &k, path, follow ? 0 : AT_SYMLINK_NOFOLLOW); assert(s.kind == kind);
}
/* Open descriptors of this process, and how many of them a program it executes would inherit. */
static void descriptors(int *opened, int *inheritable) {
    DIR *list = opendir("/proc/self/fd"); assert(list);
    *opened = *inheritable = 0;
    for (struct dirent *e; (e = readdir(list));) {
        int fd = atoi(e->d_name);
        if (e->d_name[0] == '.' || fd == dirfd(list)) continue;
        int flags = fcntl(fd, F_GETFD); assert(flags >= 0);
        ++*opened; if (fd > 2 && !(flags & FD_CLOEXEC)) ++*inheritable;
    }
    closedir(list);
}
static void touch(const char *path, const char *text) {
    int fd = open(path, O_WRONLY | O_CREAT | O_TRUNC | O_CLOEXEC, 0600);
    assert(fd >= 0 && write(fd, text, strlen(text)) == (ssize_t)strlen(text) && close(fd) == 0);
}
static int discard(const char *path, const struct stat *k, int type, struct FTW *walk) { (void)k; (void)type; (void)walk; return remove(path); }
typedef uint32_t (*text_call)(const uint8_t*, size_t, uint8_t*, size_t, size_t*);
/* read_link and real_path answer with `expected`: the text and its NUL when they fit, the length needed either way. */
static void text_is(text_call call, const char *path, const char *expected) {
    static uint8_t out[DOTNET_PAL_MAX_NAME + 2];
    size_t needed = 7, size = strlen(expected) + 1, capacity = sizeof out - 1;
    memset(out, 0xAA, sizeof out);
    assert(call(S(path), out, capacity, &needed) == 0 && needed == size && memcmp(out, expected, size) == 0 && zero(out + size, capacity - size) && out[capacity] == 0xAA);
    memset(out, 0xAA, sizeof out); needed = 7;
    assert(call(S(path), out, size, &needed) == 0 && needed == size && memcmp(out, expected, size) == 0 && out[size] == 0xAA);
    memset(out, 0xAA, sizeof out); needed = 7;
    assert(call(S(path), out, size - 1, &needed) == DOTNET_PAL_BUFFER_TOO_SMALL && needed == size && zero(out, size - 1) && out[size - 1] == 0xAA);
    needed = 7;
    assert(call(S(path), NULL, 0, &needed) == DOTNET_PAL_BUFFER_TOO_SMALL && needed == size);
}
/* The target text the kernel keeps for a link. */
static const char *target_of(const char *path) {
    static char text[DOTNET_PAL_MAX_NAME + 2];
    ssize_t size = readlink(path, text, sizeof text - 1);
    assert(size >= 0); text[size] = 0; return text;
}
/* A lock request that waits, made on a second thread. */
struct waiter { void *file; uint32_t mode, status; atomic_int started, finished; };
static void *wait_for_lock(void *argument) {
    struct waiter *w = argument;
    atomic_store(&w->started, 1);
    w->status = f->lock(w->file, w->mode, 1);
    atomic_store(&w->finished, 1);
    return NULL;
}
/* ACCESS_DENIED needs a caller the permission bits apply to. Returns 0 when the kernel lets this one through. */
static int denied(void) {
    void *handle = (void*)1; dotnet_pal_file_status s;
    int fd = open("locked/secret", O_RDONLY | O_CLOEXEC);
    if (fd >= 0 || errno != EACCES) { if (fd >= 0) close(fd); return 0; }
    assert(f->open(S("locked/secret"), R, 0, &handle) == DOTNET_PAL_ACCESS_DENIED && handle == NULL);
    REFUSED(open("locked/new", O_WRONLY | O_CREAT | O_CLOEXEC, 0600), EACCES, f->open(S("locked/new"), W | C, 0600, &handle), DOTNET_PAL_ACCESS_DENIED);
    REFUSED(stat("locked/secret", &(struct stat){0}), EACCES, f->path_status(S("locked/secret"), 1, &s, sizeof s), DOTNET_PAL_ACCESS_DENIED);
    REFUSED(unlink("locked/secret"), EACCES, f->remove(S("locked/secret")), DOTNET_PAL_ACCESS_DENIED);
    REFUSED(rename("locked/secret", "locked/moved"), EACCES, f->rename(S("locked/secret"), S("locked/moved")), DOTNET_PAL_ACCESS_DENIED);
    REFUSED(mkdir("locked/new", 0700), EACCES, f->directory_create(S("locked/new"), 0700), DOTNET_PAL_ACCESS_DENIED);
    REFUSED(opendir("locked") ? 0 : -1, EACCES, f->directory_open(S("locked"), &handle), DOTNET_PAL_ACCESS_DENIED);
    static char text[PATH_MAX]; size_t needed = 7; struct stat top;
    const struct timespec times[2] = {{1, 0}, {1, 0}};
    REFUSED(chmod("locked/secret", 0600), EACCES, f->set_mode(S("locked/secret"), 0600), DOTNET_PAL_ACCESS_DENIED);
    REFUSED(utimensat(AT_FDCWD, "locked/secret", times, 0), EACCES, f->set_times(S("locked/secret"), 1, 1000000000, 1000000000), DOTNET_PAL_ACCESS_DENIED);
    REFUSED(link("locked/secret", "locked/again"), EACCES, f->link(S("locked/secret"), S("locked/again")), DOTNET_PAL_ACCESS_DENIED);
    REFUSED(symlink("secret", "locked/link"), EACCES, f->symlink(S("secret"), S("locked/link")), DOTNET_PAL_ACCESS_DENIED);
    REFUSED(readlink("locked/secret", text, sizeof text), EACCES, f->read_link(S("locked/secret"), (uint8_t*)text, sizeof text, &needed), DOTNET_PAL_ACCESS_DENIED);
    REFUSED(realpath("locked/secret", text) ? 0 : -1, EACCES, f->real_path(S("locked/secret"), (uint8_t*)text, sizeof text, &needed), DOTNET_PAL_ACCESS_DENIED);
    REFUSED(chdir("locked"), EACCES, f->set_current_directory(S("locked")), DOTNET_PAL_ACCESS_DENIED);
    /* A node of another owner is EPERM, the same status; the mode asked for is the one it has. */
    assert(needed == 0 && stat("/", &top) == 0);
    if (top.st_uid != geteuid()) REFUSED(chmod("/", top.st_mode & 07777), EPERM, f->set_mode(S("/"), top.st_mode & 07777), DOTNET_PAL_ACCESS_DENIED);
    return 1;
}
int main(int argc, char **argv) {
#ifdef PAL_HOST_TEST
    pal_files_fault = argc > 1 ? atoi(argv[1]) : 0;
#else
    (void)argc; (void)argv;
#endif
    const dotnet_pal_api *api = dotnet_pal_get_api(2);
#ifdef PAL_HOST_TEST
    if (pal_files_fault == 1) { assert(!api); puts("FILES malformed host rejected"); return 0; }
#endif
    assert(api && api->header.struct_size >= DOTNET_PAL_FILES_API_SIZE);
    assert(api->header.capabilities & DOTNET_PAL_CAP_FILES);
    f = &api->files;
    void *file = (void*)1, *second = (void*)1, *reader = (void*)1, *directory = (void*)1;
    size_t done = 7, length = 7, needed = 7; uint32_t kind = 7;
    dotnet_pal_file_status st; dotnet_pal_files_stats before, after;
    uint8_t name[NAME + 1], data[64];
#ifdef PAL_HOST_TEST
    if (pal_files_fault == 2) {
        int handle = 0; /* any non-null handle: this provider answers before it looks at one */
        assert(f->open(S("x"), R, 0, &file) == DOTNET_PAL_OS_ERROR && file == NULL);
        assert(f->directory_open(S("x"), &directory) == DOTNET_PAL_OS_ERROR && directory == NULL);
        assert(f->read_at(&handle, 0, data, sizeof data, &done) == DOTNET_PAL_OS_ERROR && done == 0);
        done = 7;
        assert(f->write_at(&handle, 0, data, sizeof data, &done) == DOTNET_PAL_OS_ERROR && done == 0);
        /* Statuses this group does not have are not passed on. */
        assert(f->set_size(&handle, 1) == DOTNET_PAL_OS_ERROR && f->flush(&handle) == DOTNET_PAL_OS_ERROR);
        assert(f->remove(S("x")) == DOTNET_PAL_OS_ERROR && f->rename(S("x"), S("y")) == DOTNET_PAL_OS_ERROR);
        memset(&st, 0x55, sizeof st);
        assert(f->status(&handle, &st, sizeof st) == DOTNET_PAL_OS_ERROR && st.kind == 0 && st.mode == 0 && st.size == 0);
        memset(&st, 0x55, sizeof st);
        assert(f->path_status(S("x"), 1, &st, sizeof st) == DOTNET_PAL_OS_ERROR && st.kind == 0 && st.mode == 0);
        /* "." and ".." are skipped and the entry after them delivered; a malformed entry is refused and cleared. */
        memset(name, 0xAA, sizeof name);
        assert(f->directory_read(&handle, name, NAME, &length, &kind) == 0 && length == 4 && memcmp(name, "kept", 5) == 0 && kind == DOTNET_PAL_NODE_FILE);
        for (int i = 0; i < 7; ++i) {
            memset(name, 0xAA, sizeof name); length = kind = 7;
            assert(f->directory_read(&handle, name, NAME, &length, &kind) == (i < 6 ? DOTNET_PAL_OS_ERROR : DOTNET_PAL_ACCESS_DENIED));
            assert(length == 0 && kind == 0 && zero(name, NAME));
        }
        for (int i = 0; i < 4; ++i) {
            memset(data, 0xAA, sizeof data); needed = 7;
            assert(f->current_directory(data, sizeof data, &needed) == DOTNET_PAL_OS_ERROR && needed == 0 && zero(data, sizeof data));
        }
        /* The optional calls likewise: WOULD_BLOCK belongs to the two lock calls alone, BUFFER_TOO_SMALL to the calls that return text. */
        assert(f->set_mode(S("x"), 0600) == DOTNET_PAL_OS_ERROR && f->set_file_mode(&handle, 0600) == DOTNET_PAL_OS_ERROR);
        assert(f->set_times(S("x"), 1, 1, KEEP) == DOTNET_PAL_OS_ERROR && f->set_file_times(&handle, KEEP, 1) == DOTNET_PAL_OS_ERROR);
        assert(f->link(S("x"), S("y")) == DOTNET_PAL_OS_ERROR && f->symlink(S("x"), S("y")) == DOTNET_PAL_OS_ERROR);
        assert(f->set_current_directory(S("x")) == DOTNET_PAL_OS_ERROR);
        assert(f->lock(&handle, SH, 0) == DOTNET_PAL_OS_ERROR && f->lock_range(&handle, 0, 1, EX) == DOTNET_PAL_OS_ERROR);
        /* A text without its NUL, with one inside, without a length, empty, longer than any, or "too small" for a buffer it fits. */
        for (int i = 0; i < 12; ++i) {
            memset(data, 0xAA, sizeof data); needed = 7;
            assert((i < 6 ? f->read_link : f->real_path)(S("x"), data, sizeof data, &needed) == DOTNET_PAL_OS_ERROR && needed == 0 && zero(data, sizeof data));
        }
        assert(f->read_stats(&after, sizeof after) == 0 && after.directory_ok == 1 && after.rejected_or_failed == 42);
        assert(after.open_ok + after.close_ok + after.read_ok + after.write_ok + after.size_ok + after.flush_ok + after.status_ok + after.remove_ok + after.rename_ok == 0);
        assert(after.attribute_ok + after.link_ok + after.lock_ok == 0);
        puts("FILES host errors sanitized"); return 0;
    }
    if (pal_files_fault == 3) {
        /* A host without the optional callbacks keeps the capability; each of them is UNSUPPORTED, after the argument checks. */
        int handle = 0;
        assert(f->open(S("/dev/null"), R, 0, &file) == 0 && file && f->close(file) == 0);
        assert(f->set_mode(S("x"), 0600) == DOTNET_PAL_UNSUPPORTED && f->set_file_mode(&handle, 0600) == DOTNET_PAL_UNSUPPORTED);
        assert(f->set_times(S("x"), 1, 1, KEEP) == DOTNET_PAL_UNSUPPORTED && f->set_file_times(&handle, KEEP, 1) == DOTNET_PAL_UNSUPPORTED);
        assert(f->link(S("x"), S("y")) == DOTNET_PAL_UNSUPPORTED && f->symlink(S("x"), S("y")) == DOTNET_PAL_UNSUPPORTED);
        memset(data, 0xAA, sizeof data);
        assert(f->read_link(S("x"), data, sizeof data, &needed) == DOTNET_PAL_UNSUPPORTED && needed == 0 && zero(data, sizeof data));
        memset(data, 0xAA, sizeof data); needed = 7;
        assert(f->real_path(S("x"), data, sizeof data, &needed) == DOTNET_PAL_UNSUPPORTED && needed == 0 && zero(data, sizeof data));
        assert(f->set_current_directory(S("x")) == DOTNET_PAL_UNSUPPORTED);
        assert(f->lock(&handle, SH, 1) == DOTNET_PAL_UNSUPPORTED && f->lock_range(&handle, 0, 1, EX) == DOTNET_PAL_UNSUPPORTED);
        assert(f->set_mode(S("x"), 010000) == INVALID && f->lock(&handle, 0, 0) == INVALID && f->lock_range(&handle, 0, 0, SH) == INVALID);
        assert(f->read_stats(&after, sizeof after) == 0 && after.open_ok == 1 && after.close_ok == 1 && after.rejected_or_failed == 14);
        assert(after.attribute_ok + after.link_ok + after.lock_ok == 0);
        puts("FILES optional calls unsupported"); return 0;
    }
#endif
    struct stat k; int opened, inheritable, opened_now, inheritable_now;
    char root[] = "/tmp/pal-files-XXXXXX";
    umask(022);
    assert(mkdtemp(root) && chdir(root) == 0);
    descriptors(&opened, &inheritable);

    /* open: CREATE makes the file with the mode, EXCLUSIVE refuses an existing path, a missing one needs CREATE. */
    REFUSED(open("a", O_RDONLY), ENOENT, f->open(S("a"), R, 0, &file), DOTNET_PAL_NOT_FOUND);
    assert(file == NULL);
    REFUSED(open("a", O_WRONLY | O_TRUNC), ENOENT, f->open(S("a"), W | T, 0, &file), DOTNET_PAL_NOT_FOUND);
    assert(f->open(S("a"), W | C | X, 0640, &file) == 0 && file);
    assert(stat("a", &k) == 0 && S_ISREG(k.st_mode) && (k.st_mode & 07777) == 0640 && k.st_size == 0);
    REFUSED(open("a", O_WRONLY | O_CREAT | O_EXCL, 0600), EEXIST, f->open(S("a"), W | C | X, 0600, &second), DOTNET_PAL_ALREADY_EXISTS);
    assert(second == NULL);

    /* write_at: at an offset, over earlier bytes, and past the end, which leaves a hole of zeros. */
    assert(f->write_at(file, 0, (const uint8_t*)"hello world", 11, &done) == 0 && done == 11);
    assert(f->write_at(file, 4096 + 5, (const uint8_t*)"tail", 4, &done) == 0 && done == 4);
    assert(f->write_at(file, 6, (const uint8_t*)"W", 1, &done) == 0 && done == 1);
    assert(f->write_at(file, 0, data, 0, &done) == 0 && done == 0);
    assert(f->flush(file) == 0);
    static char expected[4096 + 9], actual[sizeof expected + 1];
    memcpy(expected, "hello World", 11); memcpy(expected + 4096 + 5, "tail", 4);
    int fd = open("a", O_RDONLY | O_CLOEXEC);
    assert(fd >= 0 && read(fd, actual, sizeof actual) == (ssize_t)sizeof expected && memcmp(actual, expected, sizeof expected) == 0 && close(fd) == 0);

    /* read_at: anywhere in the file, short at its end, zero bytes with OK at the end and past it. */
    assert(f->open(S("a"), R | W | C, 0600, &second) == 0 && second); /* CREATE on an existing file opens it and keeps its mode */
    assert(stat("a", &k) == 0 && (k.st_mode & 07777) == 0640);
    assert(f->read_at(second, 0, data, 5, &done) == 0 && done == 5 && memcmp(data, "hello", 5) == 0);
    assert(f->read_at(second, 4096, data, sizeof data, &done) == 0 && done == 9 && memcmp(data, "\0\0\0\0\0tail", 9) == 0);
    assert(f->read_at(second, 4096 + 9, data, sizeof data, &done) == 0 && done == 0);
    assert(f->read_at(second, UINT64_C(1) << 40, data, sizeof data, &done) == 0 && done == 0);
    assert(f->read_at(second, 0, data, 0, &done) == 0 && done == 0);

    /* A handle transfers only the way it was opened: the kernel's EBADF (EINVAL from ftruncate) is ACCESS_DENIED. */
    assert(f->open(S("a"), R, 0, &reader) == 0 && reader);
    int write_only = open("a", O_WRONLY | O_CLOEXEC), read_only = open("a", O_RDONLY | O_CLOEXEC);
    assert(write_only >= 0 && read_only >= 0);
    REFUSED(pread(write_only, data, 1, 0), EBADF, f->read_at(file, 0, data, 1, &done), DOTNET_PAL_ACCESS_DENIED);
    REFUSED(pwrite(read_only, "x", 1, 0), EBADF, f->write_at(reader, 0, (const uint8_t*)"x", 1, &done), DOTNET_PAL_ACCESS_DENIED);
    REFUSED(ftruncate(read_only, 0), EINVAL, f->set_size(reader, 0), DOTNET_PAL_ACCESS_DENIED);
    assert(done == 0 && close(write_only) == 0 && close(read_only) == 0);

    /* status describes the open file as fstat does, path_status a path as stat and lstat do. */
    assert(f->status(second, &st, sizeof st) == 0 && stat("a", &k) == 0);
    same(&st, &k, "a", 0); assert(st.kind == DOTNET_PAL_NODE_FILE && st.size == 4096 + 9 && st.mode == 0640);
    assert(symlink("a", "link") == 0 && symlink("nowhere", "dangling") == 0);
    described("a", 1, DOTNET_PAL_NODE_FILE); described("link", 1, DOTNET_PAL_NODE_FILE); described("link", 0, DOTNET_PAL_NODE_SYMLINK);
    described("dangling", 0, DOTNET_PAL_NODE_SYMLINK); described(".", 1, DOTNET_PAL_NODE_DIRECTORY); described(root, 0, DOTNET_PAL_NODE_DIRECTORY);
    described("/dev/null", 1, DOTNET_PAL_NODE_OTHER);
    assert(mkdir("sticky", 0700) == 0 && chmod("sticky", 01750) == 0);
    described("sticky", 1, DOTNET_PAL_NODE_DIRECTORY);
    assert(f->path_status(S("sticky"), 1, &st, sizeof st) == 0 && st.mode == 01750); /* all twelve mode bits, none of the type bits */
    memset(&st, 0x55, sizeof st);
    REFUSED(stat("dangling", &k), ENOENT, f->path_status(S("dangling"), 1, &st, sizeof st), DOTNET_PAL_NOT_FOUND);
    assert(st.kind == 0 && st.size == 0 && st.identity == 0);
    REFUSED(stat("a/b", &k), ENOTDIR, f->path_status(S("a/b"), 1, &st, sizeof st), DOTNET_PAL_NOT_DIRECTORY);

    /* set_size: shorter and longer, as ftruncate; TRUNCATE empties at open. */
    assert(f->set_size(second, 5) == 0 && stat("a", &k) == 0 && k.st_size == 5);
    assert(f->read_at(second, 0, data, sizeof data, &done) == 0 && done == 5 && memcmp(data, "hello", 5) == 0);
    assert(f->set_size(second, 1 << 20) == 0 && stat("a", &k) == 0 && k.st_size == 1 << 20);
    assert(f->read_at(second, (1 << 20) - 4, data, sizeof data, &done) == 0 && done == 4 && zero(data, 4));
    assert(f->close(file) == 0);
    assert(f->open(S("a"), W | T, 0, &file) == 0 && stat("a", &k) == 0 && k.st_size == 0);
    assert(f->read_at(second, 0, data, sizeof data, &done) == 0 && done == 0);
    assert(f->write_at(file, 0, (const uint8_t*)"again", 5, &done) == 0 && done == 5);

    /* Every descriptor behind a handle is close-on-exec, and a refused open leaves none behind. */
    assert(f->directory_open(S("."), &directory) == 0 && directory);
    descriptors(&opened_now, &inheritable_now);
    assert(opened_now == opened + 4 && inheritable_now == inheritable);
    assert(f->directory_close(directory) == 0);

    /* Directories: created with the mode, refused when present or without a parent, removed only when empty.
     * Linux opens a directory read-only like a file; the boundary does not. */
    assert(f->directory_create(S("sub"), 0750) == 0 && stat("sub", &k) == 0 && S_ISDIR(k.st_mode) && (k.st_mode & 07777) == 0750);
    REFUSED(mkdir("sub", 0750), EEXIST, f->directory_create(S("sub"), 0750), DOTNET_PAL_ALREADY_EXISTS);
    REFUSED(mkdir("missing/sub", 0750), ENOENT, f->directory_create(S("missing/sub"), 0750), DOTNET_PAL_NOT_FOUND);
    REFUSED(mkdir("a/sub", 0750), ENOTDIR, f->directory_create(S("a/sub"), 0750), DOTNET_PAL_NOT_DIRECTORY);
    void *inner = (void*)1;
    assert(f->open(S("sub/inner"), R | W | C | X | T, 0600, &inner) == 0 && inner && f->close(inner) == 0 && stat("sub/inner", &k) == 0);
    REFUSED(rmdir("sub"), ENOTEMPTY, f->directory_remove(S("sub")), DOTNET_PAL_NOT_EMPTY);
    REFUSED(rmdir("a"), ENOTDIR, f->directory_remove(S("a")), DOTNET_PAL_NOT_DIRECTORY);
    REFUSED(rmdir("missing"), ENOENT, f->directory_remove(S("missing")), DOTNET_PAL_NOT_FOUND);
    REFUSED(rmdir("."), EINVAL, f->directory_remove(S(".")), INVALID);
    REFUSED(rmdir("/"), EBUSY, f->directory_remove(S("/")), DOTNET_PAL_BUSY);
    fd = open("sub", O_RDONLY | O_CLOEXEC);
    assert(fd >= 0 && close(fd) == 0 && f->open(S("sub"), R, 0, &inner) == DOTNET_PAL_IS_DIRECTORY && inner == NULL);
    REFUSED(open("sub", O_RDWR), EISDIR, f->open(S("sub"), R | W, 0, &inner), DOTNET_PAL_IS_DIRECTORY);
    REFUSED(open("a/b", O_RDONLY), ENOTDIR, f->open(S("a/b"), R, 0, &inner), DOTNET_PAL_NOT_DIRECTORY);
    REFUSED(opendir("a") ? 0 : -1, ENOTDIR, f->directory_open(S("a"), &inner), DOTNET_PAL_NOT_DIRECTORY);
    REFUSED(opendir("missing") ? 0 : -1, ENOENT, f->directory_open(S("missing"), &inner), DOTNET_PAL_NOT_FOUND);
    assert(inner == NULL);

    /* remove deletes a non-directory: the link, not the file it names. */
    REFUSED(unlink("sub"), EISDIR, f->remove(S("sub")), DOTNET_PAL_IS_DIRECTORY);
    REFUSED(unlink("missing"), ENOENT, f->remove(S("missing")), DOTNET_PAL_NOT_FOUND);
    assert(f->remove(S("link")) == 0 && lstat("link", &k) == -1 && errno == ENOENT && stat("a", &k) == 0);
    assert(f->remove(S("dangling")) == 0 && f->remove(S("sub/inner")) == 0 && stat("sub/inner", &k) == -1 && errno == ENOENT);
    assert(f->directory_remove(S("sub")) == 0 && stat("sub", &k) == -1 && errno == ENOENT);

    /* rename replaces an existing destination file, and an open handle stays with the node. */
    touch("b", "old");
    assert(stat("a", &k) == 0);
    ino_t moved = k.st_ino;
    assert(f->rename(S("a"), S("b")) == 0 && stat("a", &k) == -1 && errno == ENOENT && stat("b", &k) == 0 && k.st_ino == moved && k.st_size == 5);
    assert(f->status(second, &st, sizeof st) == 0 && st.identity == moved);
    assert(f->read_at(second, 0, data, sizeof data, &done) == 0 && done == 5 && memcmp(data, "again", 5) == 0);
    assert(mkdir("empty", 0700) == 0 && mkdir("full", 0700) == 0);
    touch("full/x", "x");
    REFUSED(rename("missing", "c"), ENOENT, f->rename(S("missing"), S("c")), DOTNET_PAL_NOT_FOUND);
    REFUSED(rename("b", "empty"), EISDIR, f->rename(S("b"), S("empty")), DOTNET_PAL_IS_DIRECTORY);
    REFUSED(rename("empty", "b"), ENOTDIR, f->rename(S("empty"), S("b")), DOTNET_PAL_NOT_DIRECTORY);
    REFUSED(rename("empty", "full"), ENOTEMPTY, f->rename(S("empty"), S("full")), DOTNET_PAL_NOT_EMPTY);
    REFUSED(rename("full", "full/inside"), EINVAL, f->rename(S("full"), S("full/inside")), INVALID);
    assert(f->rename(S("full"), S("empty")) == 0 && stat("empty/x", &k) == 0 && stat("full", &k) == -1); /* a directory replaces an empty one */

    /* Names: 255 bytes is an entry, 256 is not; a path of DOTNET_PAL_MAX_NAME bytes reaches the kernel whole. */
    static char longest[NAME + 2], path[DOTNET_PAL_MAX_NAME + 2];
    memset(longest, 'n', NAME + 1);
    REFUSED(open(longest, O_RDONLY), ENAMETOOLONG, f->open(S(longest), R, 0, &inner), DOTNET_PAL_NAME_TOO_LONG);
    longest[NAME] = 0;
    for (size_t i = 0; i + 2 < DOTNET_PAL_MAX_NAME; i += 2) memcpy(path + i, "./", 2);
    path[DOTNET_PAL_MAX_NAME - 1] = 'b';
    assert(strlen(path) == DOTNET_PAL_MAX_NAME);
    described(path, 1, DOTNET_PAL_NODE_FILE);
    assert(f->open(S(path), R, 0, &inner) == 0 && f->close(inner) == 0);
    path[DOTNET_PAL_MAX_NAME] = 'b';
    assert(f->path_status(S(path), 1, &st, sizeof st) == INVALID);

    /* Enumeration: every entry once with its kind, never "." or "..", NOT_FOUND after the last and from then on. */
    assert(mkdir("list", 0700) == 0 && mkdir("list/dir", 0700) == 0 && symlink("file", "list/link") == 0 && mkfifo("list/fifo", 0600) == 0);
    static char inside[NAME + 8];
    snprintf(inside, sizeof inside, "list/%s", longest);
    touch("list/file", "f"); touch(inside, "l");
    const char *names[] = {"file", "dir", "link", "fifo", longest};
    const uint32_t kinds[] = {DOTNET_PAL_NODE_FILE, DOTNET_PAL_NODE_DIRECTORY, DOTNET_PAL_NODE_SYMLINK, DOTNET_PAL_NODE_OTHER, DOTNET_PAL_NODE_FILE};
    int seen[5] = {0}; size_t entries = 0, listed = 0;
    assert(f->directory_open(S("list"), &directory) == 0 && directory);
    for (;;) {
        memset(name, 0xAA, sizeof name); length = kind = 7;
        uint32_t status = f->directory_read(directory, name, NAME, &length, &kind);
        assert(name[NAME] == 0xAA); /* the byte after the stated capacity */
        if (status == DOTNET_PAL_NOT_FOUND) { assert(length == 0 && kind == 0 && zero(name, NAME)); break; }
        assert(status == 0 && length >= 1 && length <= NAME && zero(name + length, NAME - length));
        int index = -1;
        for (int i = 0; i < 5; ++i) if (strlen(names[i]) == length && memcmp(names[i], name, length) == 0) index = i;
        assert(index >= 0 && !seen[index]++ && kind == kinds[index]);
        ++entries;
    }
    assert(f->directory_read(directory, name, sizeof name, &length, &kind) == DOTNET_PAL_NOT_FOUND && length == 0 && kind == 0);
    DIR *list = opendir("list"); assert(list);
    for (struct dirent *e; (e = readdir(list));) if (strcmp(e->d_name, ".") != 0 && strcmp(e->d_name, "..") != 0) ++listed;
    assert(closedir(list) == 0 && entries == listed && entries == 5);
    assert(f->directory_read(NULL, name, NAME, &length, &kind) == INVALID && f->directory_read(directory, NULL, NAME, &length, &kind) == INVALID);
    assert(f->directory_read(directory, name, NAME - 1, &length, &kind) == INVALID);
    assert(f->directory_read(directory, name, NAME, NULL, &kind) == INVALID && f->directory_read(directory, name, NAME, &length, NULL) == INVALID);
    assert(f->directory_close(directory) == 0 && f->directory_close(NULL) == INVALID);
    assert(f->directory_open(S("list/dir"), &directory) == 0 && f->directory_read(directory, name, NAME, &length, &kind) == DOTNET_PAL_NOT_FOUND);
    assert(f->directory_close(directory) == 0);
    assert(f->directory_open(S(root), &directory) == 0 && f->directory_read(directory, name, NAME, &length, &kind) == 0 && f->directory_close(directory) == 0);

    /* current_directory: the text and its NUL when they fit, the length needed either way. */
    char cwd[DOTNET_PAL_MAX_NAME + 1]; static uint8_t out[DOTNET_PAL_MAX_NAME + 1];
    assert(getcwd(cwd, sizeof cwd));
    memset(out, 0xAA, sizeof out);
    assert(f->current_directory(out, sizeof out, &needed) == 0 && needed == strlen(cwd) + 1 && strcmp((const char*)out, cwd) == 0 && zero(out + needed, sizeof out - needed));
    memset(out, 0xAA, sizeof out); needed = 7;
    assert(f->current_directory(out, strlen(cwd) + 1, &needed) == 0 && needed == strlen(cwd) + 1 && strcmp((const char*)out, cwd) == 0 && out[needed] == 0xAA);
    memset(out, 0xAA, sizeof out); needed = 7;
    assert(f->current_directory(out, strlen(cwd), &needed) == DOTNET_PAL_BUFFER_TOO_SMALL && needed == strlen(cwd) + 1 && zero(out, strlen(cwd)) && out[strlen(cwd)] == 0xAA);
    needed = 7;
    assert(f->current_directory(NULL, 0, &needed) == DOTNET_PAL_BUFFER_TOO_SMALL && needed == strlen(cwd) + 1);
    assert(f->current_directory(NULL, 8, &needed) == INVALID && f->current_directory(out, sizeof out, NULL) == INVALID);

    /* set_mode and set_file_mode: the twelve bits as chmod and fchmod set them; a path that is a link names the node behind it. */
    struct stat other; void *attributes = (void*)1;
    touch("m", "mode");
    assert(symlink("m", "mlink") == 0 && lstat("mlink", &other) == 0);
    assert(f->set_mode(S("m"), 0751) == 0 && stat("m", &k) == 0 && (k.st_mode & 07777) == 0751);
    assert(f->set_mode(S("mlink"), 04640) == 0 && stat("m", &k) == 0 && (k.st_mode & 07777) == 04640);
    assert(lstat("mlink", &k) == 0 && S_ISLNK(k.st_mode) && k.st_mode == other.st_mode);
    assert(mkdir("bits", 0700) == 0 && mkdir("bits-twin", 0700) == 0 && chmod("bits-twin", 07751) == 0 && f->set_mode(S("bits"), 07751) == 0);
    assert(stat("bits", &k) == 0 && stat("bits-twin", &other) == 0 && (k.st_mode & 07777) == (other.st_mode & 07777) && (k.st_mode & 07777) != 0700);
    assert(f->open(S("m"), R, 0, &attributes) == 0 && attributes); /* a handle changes its node whatever it was opened for */
    assert(f->set_file_mode(attributes, 0604) == 0 && stat("m", &k) == 0 && (k.st_mode & 07777) == 0604);
    assert(f->status(attributes, &st, sizeof st) == 0 && st.mode == 0604 && f->set_file_mode(attributes, 0) == 0 && stat("m", &k) == 0 && (k.st_mode & 07777) == 0);
    assert(f->set_file_mode(attributes, 0600) == 0);
    REFUSED(chmod("missing", 0600), ENOENT, f->set_mode(S("missing"), 0600), DOTNET_PAL_NOT_FOUND);
    REFUSED(chmod("m/x", 0600), ENOTDIR, f->set_mode(S("m/x"), 0600), DOTNET_PAL_NOT_DIRECTORY);

    /* set_times and set_file_times: the exact nanoseconds, as utimensat and futimens; TIME_KEEP leaves that time as it
     * is; follow_links = 0 names a link itself. */
    const uint64_t t1 = UINT64_C(1234567890123456789), t2 = UINT64_C(987654321000000001), t3 = UINT64_C(1500000000999999999), t4 = 1;
    assert(f->set_times(S("m"), 1, t1, t2) == 0 && stat("m", &k) == 0 && ns(k.st_atim) == t1 && ns(k.st_mtim) == t2);
    assert(f->set_times(S("m"), 1, KEEP, t3) == 0 && stat("m", &k) == 0 && ns(k.st_atim) == t1 && ns(k.st_mtim) == t3);
    assert(f->set_times(S("m"), 1, t4, KEEP) == 0 && stat("m", &k) == 0 && ns(k.st_atim) == t4 && ns(k.st_mtim) == t3);
    assert(f->set_times(S("m"), 1, KEEP, KEEP) == 0 && stat("m", &k) == 0 && ns(k.st_atim) == t4 && ns(k.st_mtim) == t3);
    assert(f->path_status(S("m"), 1, &st, sizeof st) == 0 && st.accessed_ns == t4 && st.modified_ns == t3);
    assert(f->set_times(S("mlink"), 1, t2, t1) == 0 && stat("m", &k) == 0 && ns(k.st_atim) == t2 && ns(k.st_mtim) == t1);
    assert(f->set_times(S("mlink"), 0, t3, t4) == 0 && lstat("mlink", &k) == 0 && S_ISLNK(k.st_mode) && ns(k.st_atim) == t3 && ns(k.st_mtim) == t4);
    assert(stat("m", &k) == 0 && ns(k.st_atim) == t2 && ns(k.st_mtim) == t1);
    assert(symlink("nowhere", "mdangling") == 0 && f->set_times(S("mdangling"), 0, t1, t2) == 0 && lstat("mdangling", &k) == 0 && ns(k.st_atim) == t1 && ns(k.st_mtim) == t2);
    const struct timespec kernel_times[2] = {{1, 0}, {0, UTIME_OMIT}};
    REFUSED(utimensat(AT_FDCWD, "mdangling", kernel_times, 0), ENOENT, f->set_times(S("mdangling"), 1, t1, KEEP), DOTNET_PAL_NOT_FOUND);
    REFUSED(utimensat(AT_FDCWD, "missing", kernel_times, AT_SYMLINK_NOFOLLOW), ENOENT, f->set_times(S("missing"), 0, t1, KEEP), DOTNET_PAL_NOT_FOUND);
    assert(f->set_file_times(attributes, t3, KEEP) == 0 && stat("m", &k) == 0 && ns(k.st_atim) == t3 && ns(k.st_mtim) == t1);
    assert(f->set_file_times(attributes, KEEP, t4) == 0 && stat("m", &k) == 0 && ns(k.st_atim) == t3 && ns(k.st_mtim) == t4);
    assert(f->set_file_times(attributes, 0, 0) == 0 && f->status(attributes, &st, sizeof st) == 0 && st.accessed_ns == 0 && st.modified_ns == 0);
    /* The latest time the boundary takes is whatever the kernel makes of it on this filesystem. */
    const struct timespec latest[2] = {{INT64_MAX / 1000000000, INT64_MAX % 1000000000}, {0, UTIME_OMIT}};
    touch("twin", "");
    assert(utimensat(AT_FDCWD, "twin", latest, 0) == 0 && f->set_times(S("m"), 1, INT64_MAX, KEEP) == 0 && stat("twin", &other) == 0 && stat("m", &k) == 0);
    assert(k.st_atim.tv_sec == other.st_atim.tv_sec && k.st_atim.tv_nsec == other.st_atim.tv_nsec && ns(k.st_mtim) == 0);
    assert(f->close(attributes) == 0);

    /* link: a second name of the same node, as link(2); never over an existing name. */
    assert(f->link(S("m"), S("hard")) == 0 && stat("m", &k) == 0 && lstat("hard", &other) == 0 && S_ISREG(other.st_mode));
    assert(k.st_ino == other.st_ino && k.st_dev == other.st_dev && k.st_nlink == 2);
    REFUSED(link("m", "hard"), EEXIST, f->link(S("m"), S("hard")), DOTNET_PAL_ALREADY_EXISTS);
    REFUSED(link("missing", "hard2"), ENOENT, f->link(S("missing"), S("hard2")), DOTNET_PAL_NOT_FOUND);
    REFUSED(link("m", "missing/hard"), ENOENT, f->link(S("m"), S("missing/hard")), DOTNET_PAL_NOT_FOUND);
    REFUSED(link("m", "m/hard"), ENOTDIR, f->link(S("m"), S("m/hard")), DOTNET_PAL_NOT_DIRECTORY);
    REFUSED(link("list", "hard-directory"), EPERM, f->link(S("list"), S("hard-directory")), DOTNET_PAL_ACCESS_DENIED);
    assert(lstat("hard2", &k) == -1 && f->remove(S("hard")) == 0 && stat("m", &k) == 0 && k.st_nlink == 1);

    /* symlink stores the target text as given, whether or not it names anything, and read_link gives it back:
     * the text and its NUL when they fit, the length needed either way. */
    static char target300[301], longest_target[DOTNET_PAL_MAX_NAME + 1], resolved[PATH_MAX], canonical[PATH_MAX + 8];
    memset(target300, 'x', 300); memset(longest_target, 't', DOTNET_PAL_MAX_NAME);
    assert(f->symlink(S("m"), S("s")) == 0 && lstat("s", &k) == 0 && S_ISLNK(k.st_mode) && stat("s", &other) == 0 && S_ISREG(other.st_mode));
    assert(strcmp(target_of("s"), "m") == 0);
    text_is(f->read_link, "s", "m"); described("s", 0, DOTNET_PAL_NODE_SYMLINK);
    assert(f->symlink(S("no/../such//target/"), S("s-dangling")) == 0 && strcmp(target_of("s-dangling"), "no/../such//target/") == 0);
    text_is(f->read_link, "s-dangling", "no/../such//target/");
    assert(f->symlink(S(target300), S("s-300")) == 0 && strcmp(target_of("s-300"), target300) == 0);
    text_is(f->read_link, "s-300", target300);
    /* The longest text the boundary carries is the longest Linux stores; a filesystem may stop earlier. */
    int longest_link = symlink(longest_target, "k-longest") == 0;
    assert(f->symlink(S(longest_target), S("s-longest")) == (longest_link ? DOTNET_PAL_OK : DOTNET_PAL_NAME_TOO_LONG));
    if (longest_link) { assert(strcmp(target_of("s-longest"), longest_target) == 0); text_is(f->read_link, "s-longest", longest_target); }
    REFUSED(symlink("m", "s"), EEXIST, f->symlink(S("m"), S("s")), DOTNET_PAL_ALREADY_EXISTS);
    REFUSED(symlink("m", "missing/s"), ENOENT, f->symlink(S("m"), S("missing/s")), DOTNET_PAL_NOT_FOUND);
    memset(out, 0xAA, sizeof out); needed = 7;
    REFUSED(readlink("m", resolved, sizeof resolved), EINVAL, f->read_link(S("m"), out, sizeof out, &needed), INVALID); /* not a link */
    assert(needed == 0 && zero(out, sizeof out));
    REFUSED(readlink("list", resolved, sizeof resolved), EINVAL, f->read_link(S("list"), out, sizeof out, &needed), INVALID);
    REFUSED(readlink("missing", resolved, sizeof resolved), ENOENT, f->read_link(S("missing"), out, sizeof out, &needed), DOTNET_PAL_NOT_FOUND);
    REFUSED(readlink("m/x", resolved, sizeof resolved), ENOTDIR, f->read_link(S("m/x"), out, sizeof out, &needed), DOTNET_PAL_NOT_DIRECTORY);

    /* real_path: absolute, with every link, "." and ".." resolved, as realpath(3). */
    assert(snprintf(canonical, sizeof canonical, "%s/m", cwd) > 0);
    assert(realpath("list/../s", resolved) && strcmp(resolved, canonical) == 0);
    text_is(f->real_path, "list/../s", canonical); text_is(f->real_path, "./list/./dir/..//", realpath("list", resolved));
    text_is(f->real_path, ".", cwd); text_is(f->real_path, "/", "/"); text_is(f->real_path, canonical, canonical);
    memset(out, 0xAA, sizeof out); needed = 7;
    REFUSED(realpath("missing", resolved) ? 0 : -1, ENOENT, f->real_path(S("missing"), out, sizeof out, &needed), DOTNET_PAL_NOT_FOUND);
    assert(needed == 0 && zero(out, sizeof out));
    REFUSED(realpath("s-dangling", resolved) ? 0 : -1, ENOENT, f->real_path(S("s-dangling"), out, sizeof out, &needed), DOTNET_PAL_NOT_FOUND);
    REFUSED(realpath("m/x", resolved) ? 0 : -1, ENOTDIR, f->real_path(S("m/x"), out, sizeof out, &needed), DOTNET_PAL_NOT_DIRECTORY);

    /* set_current_directory moves what current_directory reports and what a relative path means. */
    assert(f->set_current_directory(S("list")) == 0 && getcwd(resolved, sizeof resolved));
    assert(strlen(resolved) == strlen(cwd) + 5 && memcmp(resolved, cwd, strlen(cwd)) == 0 && strcmp(resolved + strlen(cwd), "/list") == 0);
    assert(f->current_directory(out, sizeof out, &needed) == 0 && strcmp((const char*)out, resolved) == 0);
    assert(f->open(S("file"), R, 0, &inner) == 0 && inner && f->close(inner) == 0 && f->path_status(S("m"), 1, &st, sizeof st) == DOTNET_PAL_NOT_FOUND);
    described("dir", 1, DOTNET_PAL_NODE_DIRECTORY); described("../m", 1, DOTNET_PAL_NODE_FILE);
    REFUSED(chdir("missing"), ENOENT, f->set_current_directory(S("missing")), DOTNET_PAL_NOT_FOUND);
    REFUSED(chdir("file"), ENOTDIR, f->set_current_directory(S("file")), DOTNET_PAL_NOT_DIRECTORY);
    assert(f->set_current_directory(S("..")) == 0 && getcwd(resolved, sizeof resolved) && strcmp(resolved, cwd) == 0);
    assert(symlink("list/dir", "to-dir") == 0 && f->set_current_directory(S("to-dir")) == 0 && getcwd(resolved, sizeof resolved)); /* through a link: where it leads */
    assert(strcmp(resolved + strlen(cwd), "/list/dir") == 0 && f->set_current_directory(S(cwd)) == 0 && getcwd(resolved, sizeof resolved) && strcmp(resolved, cwd) == 0);

    /* lock: advisory, on the whole file, held by the handle. The kernel's side of each answer is a descriptor of its
     * own on the same file. A handle that holds a lock never asks for another here: flock gives up the old one first. */
    void *l1 = (void*)1, *l2 = (void*)1, *l3 = (void*)1;
    touch("lk", "0123456789");
    assert(f->open(S("lk"), R | W, 0, &l1) == 0 && f->open(S("lk"), R | W, 0, &l2) == 0 && f->open(S("lk"), R, 0, &l3) == 0);
    int probe = open("lk", O_RDWR | O_CLOEXEC);
    assert(probe >= 0 && f->lock(l1, SH, 0) == 0 && f->lock(l2, SH, 0) == 0); /* shared locks coexist, */
    assert(flock(probe, LOCK_SH | LOCK_NB) == 0 && flock(probe, LOCK_UN) == 0); /* with the kernel's own too, */
    REFUSED(flock(probe, LOCK_EX | LOCK_NB), EWOULDBLOCK, f->lock(l3, EX, 0), DOTNET_PAL_WOULD_BLOCK); /* and exclude an exclusive one */
    assert(f->lock(l1, UN, 0) == 0);
    REFUSED(flock(probe, LOCK_EX | LOCK_NB), EWOULDBLOCK, f->lock(l3, EX, 0), DOTNET_PAL_WOULD_BLOCK); /* one of the two is left */
    assert(f->lock(l2, UN, 0) == 0 && f->lock(l3, EX, 0) == 0); /* UNLOCK releases; write access is not needed */
    REFUSED(flock(probe, LOCK_SH | LOCK_NB), EWOULDBLOCK, f->lock(l1, SH, 0), DOTNET_PAL_WOULD_BLOCK);
    assert(f->lock(l2, EX, 0) == DOTNET_PAL_WOULD_BLOCK && f->lock(l2, UN, 0) == 0); /* UNLOCK without a lock is nothing to do, */
    REFUSED(flock(probe, LOCK_SH | LOCK_NB), EWOULDBLOCK, f->lock(l2, SH, 0), DOTNET_PAL_WOULD_BLOCK); /* and takes nobody else's */
    assert(f->close(l3) == 0 && f->lock(l1, EX, 0) == 0); /* close releases */
    /* wait = 1 waits: the second thread's request returns only once the holder unlocks, and then it holds the lock. */
    struct waiter waiter = {.file = l2, .mode = EX, .status = 99};
    pthread_t thread;
    assert(pthread_create(&thread, NULL, wait_for_lock, &waiter) == 0);
    while (!atomic_load(&waiter.started)) sched_yield();
    nanosleep(&(struct timespec){0, 200000000}, NULL);
    assert(!atomic_load(&waiter.finished) && f->lock(l1, UN, 0) == 0);
    assert(pthread_join(thread, NULL) == 0 && atomic_load(&waiter.finished) && waiter.status == 0);
    REFUSED(flock(probe, LOCK_SH | LOCK_NB), EWOULDBLOCK, f->lock(l1, SH, 0), DOTNET_PAL_WOULD_BLOCK);
    assert(f->lock(l2, UN, 1) == 0 && f->lock(l1, SH, 1) == 0 && f->lock(l1, UN, 0) == 0); /* a free lock does not wait */

    /* lock_range: bytes [offset, offset + length), held by the handle, never waiting. To the kernel it is an
     * open-file-description lock, which F_OFD_GETLK reports without a process; a lock of the process would let the
     * second handle through. */
    struct flock range = {.l_type = F_WRLCK, .l_whence = SEEK_SET, .l_start = 0, .l_len = 1};
    assert(f->open(S("lk"), R | W, 0, &l3) == 0 && f->lock_range(l1, 0, 10, EX) == 0 && f->lock_range(l2, 10, 10, EX) == 0); /* disjoint ranges */
    assert(fcntl(probe, F_OFD_GETLK, &range) == 0 && range.l_type == F_WRLCK && range.l_pid == -1 && range.l_start == 0 && range.l_len == 10);
    range = (struct flock){.l_type = F_RDLCK, .l_whence = SEEK_SET, .l_start = 9, .l_len = 1};
    REFUSED(fcntl(probe, F_OFD_SETLK, &range), EAGAIN, f->lock_range(l3, 9, 1, SH), DOTNET_PAL_WOULD_BLOCK); /* overlapping ones */
    assert(f->lock_range(l3, 5, 10, EX) == DOTNET_PAL_WOULD_BLOCK && f->lock_range(l3, 19, 5, SH) == DOTNET_PAL_WOULD_BLOCK);
    assert(f->lock_range(l3, 20, 5, EX) == 0 && f->lock_range(l3, 20, 5, UN) == 0 && f->lock_range(l1, 0, 10, UN) == 0 && f->lock_range(l2, 10, 10, UN) == 0);
    assert(f->lock_range(l1, 0, 10, SH) == 0 && f->lock_range(l2, 5, 10, SH) == 0); /* shared ranges coexist and exclude an exclusive one */
    range = (struct flock){.l_type = F_WRLCK, .l_whence = SEEK_SET, .l_start = 12, .l_len = 1};
    REFUSED(fcntl(probe, F_OFD_SETLK, &range), EAGAIN, f->lock_range(l3, 12, 1, EX), DOTNET_PAL_WOULD_BLOCK);
    assert(f->lock_range(l3, 7, 1, EX) == DOTNET_PAL_WOULD_BLOCK && f->lock_range(l3, 15, 1, EX) == 0); /* the byte after the range is free */
    assert(f->lock_range(l1, (uint64_t)INT64_MAX - 1, 1, EX) == 0 && f->lock_range(l3, (uint64_t)INT64_MAX - 1, 1, SH) == DOTNET_PAL_WOULD_BLOCK); /* the last byte there is */
    assert(f->close(l1) == 0 && f->lock_range(l3, 0, 5, EX) == 0 && f->lock_range(l3, (uint64_t)INT64_MAX - 1, 1, EX) == 0); /* close releases */
    assert(f->lock_range(l3, 5, 1, EX) == DOTNET_PAL_WOULD_BLOCK && f->close(l2) == 0 && f->lock_range(l3, 0, INT64_MAX, EX) == 0);
    /* The kernel wants read access for a shared range and write access for an exclusive one: the handle was not opened for it. */
    void *reading = (void*)1, *writing = (void*)1;
    int read_only_lock = open("lk", O_RDONLY | O_CLOEXEC), write_only_lock = open("lk", O_WRONLY | O_CLOEXEC);
    assert(read_only_lock >= 0 && write_only_lock >= 0 && f->open(S("lk"), R, 0, &reading) == 0 && f->open(S("lk"), W, 0, &writing) == 0);
    assert(f->lock_range(l3, 0, INT64_MAX, UN) == 0);
    range = (struct flock){.l_type = F_WRLCK, .l_whence = SEEK_SET, .l_start = 100, .l_len = 1};
    REFUSED(fcntl(read_only_lock, F_OFD_SETLK, &range), EBADF, f->lock_range(reading, 100, 1, EX), DOTNET_PAL_ACCESS_DENIED);
    range.l_type = F_RDLCK;
    REFUSED(fcntl(write_only_lock, F_OFD_SETLK, &range), EBADF, f->lock_range(writing, 100, 1, SH), DOTNET_PAL_ACCESS_DENIED);
    assert(f->lock_range(reading, 100, 1, SH) == 0 && f->lock_range(writing, 101, 1, EX) == 0 && f->lock_range(l3, 100, 2, EX) == DOTNET_PAL_WOULD_BLOCK);
    assert(f->close(reading) == 0 && f->close(writing) == 0 && f->lock_range(l3, 100, 2, EX) == 0);
    assert(f->close(l3) == 0 && close(probe) == 0 && close(read_only_lock) == 0 && close(write_only_lock) == 0);

    /* Conditions that depend on where the test runs: checked when the kernel itself reports them here. */
    int no_space = 0, read_only_mount = 0, cross_device = 0, access_denied = 0;
    fd = open("/dev/full", O_WRONLY | O_CLOEXEC);
    if (fd >= 0 && pwrite(fd, "x", 1, 0) == -1 && errno == ENOSPC) {
        assert(f->open(S("/dev/full"), W, 0, &inner) == 0 && f->write_at(inner, 0, (const uint8_t*)"x", 1, &done) == DOTNET_PAL_NO_SPACE && done == 0 && f->close(inner) == 0);
        no_space = 1;
    }
    if (fd >= 0) close(fd);
    fd = open("/proc/sys/kernel/hostname", O_WRONLY | O_CLOEXEC);
    if (fd == -1 && errno == EROFS) { assert(f->open(S("/proc/sys/kernel/hostname"), W, 0, &inner) == DOTNET_PAL_READ_ONLY && inner == NULL); read_only_mount = 1; }
    if (fd >= 0) close(fd);
    char elsewhere[64];
    snprintf(elsewhere, sizeof elsewhere, "/dev/shm/pal-files-%ld", (long)getpid());
    touch("probe", "p");
    if (rename("probe", elsewhere) == -1 && errno == EXDEV) {
        assert(f->rename(S("probe"), S(elsewhere)) == DOTNET_PAL_CROSS_DEVICE && stat("probe", &k) == 0);
        REFUSED(link("probe", elsewhere), EXDEV, f->link(S("probe"), S(elsewhere)), DOTNET_PAL_CROSS_DEVICE);
        cross_device = 1;
    }
    unlink(elsewhere);
    struct rlimit limit, none;
    assert(getrlimit(RLIMIT_NOFILE, &limit) == 0);
    none = limit; none.rlim_cur = 0;
    assert(setrlimit(RLIMIT_NOFILE, &none) == 0);
    REFUSED(open("b", O_RDONLY), EMFILE, f->open(S("b"), R, 0, &inner), DOTNET_PAL_TOO_MANY_HANDLES);
    REFUSED(opendir(".") ? 0 : -1, EMFILE, f->directory_open(S("."), &directory), DOTNET_PAL_TOO_MANY_HANDLES);
    assert(setrlimit(RLIMIT_NOFILE, &limit) == 0 && inner == NULL && directory == NULL);
    /* Root passes every permission check, so a root run asks as a child that gave its privileges up. */
    assert(mkdir("locked", 0700) == 0);
    touch("locked/secret", "s");
    assert(chmod("locked", 0) == 0);
    if (geteuid() != 0) access_denied = denied();
    else {
        pid_t child = fork(); assert(child >= 0);
        if (child == 0) _exit(setgroups(0, NULL) == 0 && setgid(65534) == 0 && setuid(65534) == 0 && denied() ? 0 : 77);
        int result = 0;
        assert(waitpid(child, &result, 0) == child && WIFEXITED(result) && (WEXITSTATUS(result) == 0 || WEXITSTATUS(result) == 77));
        access_denied = WEXITSTATUS(result) == 0;
    }
    assert(chmod("locked", 0700) == 0);

    /* Argument validation: refused before a provider runs, outputs cleared. */
    static char oversized[DOTNET_PAL_MAX_NAME + 2];
    memset(oversized, 'p', DOTNET_PAL_MAX_NAME + 1);
    const uint32_t flags[] = {0, C, R | X, W | X, R | T, R | C | T, R | 32u, 0x80000001u};
    for (size_t i = 0; i < sizeof flags / sizeof *flags; ++i) { inner = (void*)1; assert(f->open(S("b"), flags[i], 0, &inner) == INVALID && inner == NULL); }
    inner = (void*)1;
    assert(f->open(NULL, 1, R, 0, &inner) == INVALID && inner == NULL && f->open((const uint8_t*)"", 0, R, 0, &inner) == INVALID);
    assert(f->open((const uint8_t*)"b\0c", 3, R, 0, &inner) == INVALID && f->open(S(oversized), R, 0, &inner) == INVALID);
    assert(f->open(S("b"), R, 0, NULL) == INVALID && f->open(S("b"), R, 010000, &inner) == INVALID);
    assert(f->path_status(NULL, 1, 1, &st, sizeof st) == INVALID && f->path_status(S("b"), 2, &st, sizeof st) == INVALID);
    assert(f->path_status(S("b"), 1, NULL, sizeof st) == INVALID && f->path_status(S("b"), 1, &st, sizeof st - 1) == INVALID);
    assert(f->remove(NULL, 1) == INVALID && f->remove((const uint8_t*)"b\0", 2) == INVALID && f->remove((const uint8_t*)"b", 0) == INVALID && stat("b", &k) == 0);
    assert(f->rename(NULL, 1, S("c")) == INVALID && f->rename(S("b"), NULL, 1) == INVALID && f->rename(S("b"), (const uint8_t*)"c\0d", 3) == INVALID && stat("b", &k) == 0);
    assert(f->directory_create(NULL, 1, 0700) == INVALID && f->directory_create(S("c"), 010000) == INVALID && stat("c", &k) == -1);
    assert(f->directory_remove(NULL, 1) == INVALID && f->directory_remove((const uint8_t*)"", 0) == INVALID);
    directory = (void*)1;
    assert(f->directory_open(NULL, 1, &directory) == INVALID && directory == NULL && f->directory_open(S("."), NULL) == INVALID);
    assert(f->close(NULL) == INVALID && f->flush(NULL) == INVALID && f->set_size(NULL, 0) == INVALID && f->status(NULL, &st, sizeof st) == INVALID);
    assert(f->read_at(NULL, 0, data, 1, &done) == INVALID && f->read_at(second, 0, NULL, 1, &done) == INVALID && f->read_at(second, 0, data, 1, NULL) == INVALID);
    assert(f->read_at(second, (uint64_t)INT64_MAX + 1, data, 1, &done) == INVALID);
    assert(f->write_at(NULL, 0, data, 1, &done) == INVALID && f->write_at(second, 0, NULL, 1, &done) == INVALID && f->write_at(second, 0, data, 1, NULL) == INVALID);
    assert(f->write_at(second, (uint64_t)INT64_MAX + 1, data, 1, &done) == INVALID && f->write_at(second, INT64_MAX, data, 1, &done) == INVALID);
    assert(f->set_size(second, (uint64_t)INT64_MAX + 1) == INVALID && stat("b", &k) == 0 && k.st_size == 5);
    assert(f->status(second, NULL, sizeof st) == INVALID && f->status(second, &st, sizeof st - 1) == INVALID);
    assert(f->read_stats(NULL, sizeof after) == INVALID && f->read_stats(&after, sizeof after - 1) == INVALID);
    assert(stat("b", &other) == 0);
    assert(f->set_mode(NULL, 1, 0600) == INVALID && f->set_mode((const uint8_t*)"", 0, 0600) == INVALID && f->set_mode((const uint8_t*)"b\0", 2, 0600) == INVALID);
    assert(f->set_mode(S("b"), 010000) == INVALID && f->set_mode(S(oversized), 0600) == INVALID && f->set_file_mode(NULL, 0600) == INVALID && f->set_file_mode(second, 010000) == INVALID);
    assert(f->set_times(NULL, 1, 1, 1, 1) == INVALID && f->set_times((const uint8_t*)"", 0, 1, 1, 1) == INVALID && f->set_times(S("b"), 2, 1, 1) == INVALID);
    assert(f->set_times(S("b"), 1, (uint64_t)INT64_MAX + 1, KEEP) == INVALID && f->set_times(S("b"), 1, KEEP, KEEP - 1) == INVALID); /* past the range, and not TIME_KEEP */
    assert(f->set_file_times(NULL, 1, 1) == INVALID && f->set_file_times(second, (uint64_t)INT64_MAX + 1, 1) == INVALID && f->set_file_times(second, 1, (uint64_t)INT64_MAX + 1) == INVALID);
    assert(stat("b", &k) == 0 && k.st_mode == other.st_mode && ns(k.st_atim) == ns(other.st_atim) && ns(k.st_mtim) == ns(other.st_mtim));
    assert(f->link(NULL, 1, S("c")) == INVALID && f->link(S("b"), NULL, 1) == INVALID && f->link(S("b"), (const uint8_t*)"", 0) == INVALID && f->link(S("b"), (const uint8_t*)"c\0d", 3) == INVALID);
    assert(f->symlink(NULL, 1, S("c")) == INVALID && f->symlink(S("b"), NULL, 1) == INVALID && f->symlink((const uint8_t*)"", 0, S("c")) == INVALID);
    assert(f->symlink((const uint8_t*)"b\0", 2, S("c")) == INVALID && f->symlink(S(oversized), S("c")) == INVALID && lstat("c", &k) == -1);
    needed = 7;
    assert(f->read_link(NULL, 1, out, sizeof out, &needed) == INVALID && needed == 0 && f->read_link((const uint8_t*)"s\0", 2, out, sizeof out, &needed) == INVALID);
    assert(f->read_link(S("s"), NULL, 8, &needed) == INVALID && f->read_link(S("s"), out, sizeof out, NULL) == INVALID);
    needed = 7;
    assert(f->real_path(NULL, 1, out, sizeof out, &needed) == INVALID && needed == 0 && f->real_path((const uint8_t*)"", 0, out, sizeof out, &needed) == INVALID);
    assert(f->real_path(S("s"), NULL, 8, &needed) == INVALID && f->real_path(S("s"), out, sizeof out, NULL) == INVALID);
    assert(f->set_current_directory(NULL, 1) == INVALID && f->set_current_directory((const uint8_t*)"", 0) == INVALID && f->set_current_directory((const uint8_t*)"list\0", 5) == INVALID);
    assert(getcwd(resolved, sizeof resolved) && strcmp(resolved, cwd) == 0);
    assert(f->lock(NULL, SH, 0) == INVALID && f->lock(second, 0, 0) == INVALID && f->lock(second, 4, 0) == INVALID && f->lock(second, EX, 2) == INVALID);
    assert(f->lock_range(NULL, 0, 1, SH) == INVALID && f->lock_range(second, 0, 1, 0) == INVALID && f->lock_range(second, 0, 1, 4) == INVALID && f->lock_range(second, 0, 0, EX) == INVALID);
    assert(f->lock_range(second, (uint64_t)INT64_MAX + 1, 1, EX) == INVALID && f->lock_range(second, INT64_MAX, 1, EX) == INVALID && f->lock_range(second, 1, INT64_MAX, EX) == INVALID);
    assert(f->lock_range(second, UINT64_MAX, UINT64_MAX, EX) == INVALID);
    fd = open("b", O_RDWR | O_CLOEXEC); /* none of them took a lock */
    struct flock all = {.l_type = F_WRLCK, .l_whence = SEEK_SET};
    assert(fd >= 0 && flock(fd, LOCK_EX | LOCK_NB) == 0 && fcntl(fd, F_OFD_SETLK, &all) == 0 && close(fd) == 0);

    /* Counters: one per class of successful operation, one for everything refused. */
    assert(f->close(file) == 0 && f->close(second) == 0 && f->close(reader) == 0);
    assert(f->read_stats(&before, sizeof before) == 0);
    assert(f->open(S("counted"), R | W | C, 0600, &file) == 0 && f->write_at(file, 0, (const uint8_t*)"12", 2, &done) == 0 && f->read_at(file, 0, data, 2, &done) == 0);
    assert(f->set_size(file, 1) == 0 && f->flush(file) == 0 && f->status(file, &st, sizeof st) == 0 && f->path_status(S("counted"), 1, &st, sizeof st) == 0);
    assert(f->set_mode(S("counted"), 0640) == 0 && f->set_file_mode(file, 0600) == 0 && f->set_times(S("counted"), 1, 1, KEEP) == 0 && f->set_file_times(file, KEEP, 1) == 0);
    assert(f->link(S("counted"), S("counted-hard")) == 0 && f->symlink(S("counted"), S("counted-link")) == 0);
    assert(f->read_link(S("counted-link"), out, sizeof out, &needed) == 0 && f->real_path(S("counted-link"), out, sizeof out, &needed) == 0);
    assert(f->read_link(S("counted-link"), out, 1, &needed) == DOTNET_PAL_BUFFER_TOO_SMALL && unlink("counted-hard") == 0 && unlink("counted-link") == 0);
    fd = open("counted", O_RDONLY | O_CLOEXEC);
    assert(fd >= 0 && flock(fd, LOCK_EX | LOCK_NB) == 0 && f->lock(file, SH, 0) == DOTNET_PAL_WOULD_BLOCK && close(fd) == 0);
    assert(f->lock(file, SH, 0) == 0 && f->lock(file, UN, 0) == 0 && f->lock_range(file, 0, 1, EX) == 0 && f->lock_range(file, 0, 1, UN) == 0);
    assert(f->close(file) == 0 && f->rename(S("counted"), S("recounted")) == 0 && f->remove(S("recounted")) == 0);
    assert(f->directory_create(S("counted"), 0700) == 0 && f->directory_open(S("counted"), &directory) == 0);
    assert(f->directory_read(directory, name, NAME, &length, &kind) == DOTNET_PAL_NOT_FOUND && f->directory_close(directory) == 0);
    assert(f->directory_remove(S("counted")) == 0 && f->current_directory(out, sizeof out, &needed) == 0 && f->set_current_directory(S(".")) == 0);
    assert(f->current_directory(out, 1, &needed) == DOTNET_PAL_BUFFER_TOO_SMALL && f->flush(NULL) == INVALID);
    assert(f->read_stats(&after, sizeof after) == 0);
    assert(after.open_ok == before.open_ok + 1 && after.close_ok == before.close_ok + 1 && after.read_ok == before.read_ok + 1 && after.write_ok == before.write_ok + 1);
    assert(after.size_ok == before.size_ok + 1 && after.flush_ok == before.flush_ok + 1 && after.status_ok == before.status_ok + 2);
    assert(after.remove_ok == before.remove_ok + 1 && after.rename_ok == before.rename_ok + 1 && after.directory_ok == before.directory_ok + 6);
    assert(after.attribute_ok == before.attribute_ok + 4 && after.link_ok == before.link_ok + 4 && after.lock_ok == before.lock_ok + 4);
    assert(after.rejected_or_failed == before.rejected_or_failed + 5);

    /* Nothing the boundary opened is still open, and the scratch directory goes away. */
    descriptors(&opened_now, &inheritable_now);
    assert(opened_now == opened && inheritable_now == inheritable);
    assert(chdir("/") == 0 && nftw(root, discard, 16, FTW_DEPTH | FTW_PHYS) == 0 && stat(root, &k) == -1 && errno == ENOENT);
    printf("FILES PASS entries=%zu access_denied=%d no_space=%d read_only=%d cross_device=%d longest_link=%d\n", entries, access_denied, no_space, read_only_mount, cross_device, longest_link);
    return 0;
}

/* Independent POSIX reference provider for the host-files conformance suite.
 * Fault 1 withholds a callback; fault 2 breaks the output contracts so the
 * front end's sanitizing is observable; fault 3 is a host without the optional
 * callbacks. A file handle is the descriptor plus one, a directory handle the
 * C library's stream. */
#define _GNU_SOURCE
#include "dotnet_pal.h"
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/file.h>
#include <sys/stat.h>
#include <sys/sysmacros.h>
#include <unistd.h>
int pal_files_fault;
static uint32_t failure(void) {
    switch (errno) {
    case ENOENT: return DOTNET_PAL_NOT_FOUND;
    case EEXIST: return DOTNET_PAL_ALREADY_EXISTS;
    case EACCES: case EPERM: return DOTNET_PAL_ACCESS_DENIED;
    case EISDIR: return DOTNET_PAL_IS_DIRECTORY;
    case ENOTDIR: return DOTNET_PAL_NOT_DIRECTORY;
    case ENOTEMPTY: return DOTNET_PAL_NOT_EMPTY;
    case ENOSPC: case EDQUOT: return DOTNET_PAL_NO_SPACE;
    case EMFILE: case ENFILE: return DOTNET_PAL_TOO_MANY_HANDLES;
    case ENAMETOOLONG: return DOTNET_PAL_NAME_TOO_LONG;
    case EROFS: return DOTNET_PAL_READ_ONLY;
    case EXDEV: return DOTNET_PAL_CROSS_DEVICE;
    case ENOMEM: return DOTNET_PAL_OUT_OF_MEMORY;
    case EINVAL: return DOTNET_PAL_INVALID_ARGUMENT;
    case EBUSY: return DOTNET_PAL_BUSY;
    default: return DOTNET_PAL_OS_ERROR;
    }
}
static uint32_t result(int rc) { return rc == 0 ? DOTNET_PAL_OK : failure(); }
/* Paths are byte borrows without a terminator. */
static int terminated(char *out, const uint8_t *path, size_t length) {
    if (length >= PATH_MAX) return 0;
    memcpy(out, path, length); out[length] = 0; return 1;
}
static int descriptor(void *file) { return (int)(intptr_t)file - 1; }
/* For the host providers of the groups that take the handles of this one (tests/mappings_host.c). */
int pal_files_host_descriptor(void *file) { return descriptor(file); }
static uint64_t nanoseconds(int64_t seconds, uint32_t nanos) { return seconds < 0 ? 0 : (uint64_t)seconds * UINT64_C(1000000000) + nanos; }
static uint32_t node(mode_t mode) {
    return S_ISREG(mode) ? DOTNET_PAL_NODE_FILE : S_ISDIR(mode) ? DOTNET_PAL_NODE_DIRECTORY : S_ISLNK(mode) ? DOTNET_PAL_NODE_SYMLINK : DOTNET_PAL_NODE_OTHER;
}
/* One statx answers everything, the birth time included when the filesystem keeps it. */
static uint32_t describe(int directory, const char *path, int flags, dotnet_pal_file_status *out) {
    struct statx x;
    if (statx(directory, path, flags, STATX_BASIC_STATS | STATX_BTIME, &x) != 0) return failure();
    out->kind = node(x.stx_mode); out->mode = x.stx_mode & 07777u; out->size = x.stx_size;
    out->modified_ns = nanoseconds(x.stx_mtime.tv_sec, x.stx_mtime.tv_nsec); out->accessed_ns = nanoseconds(x.stx_atime.tv_sec, x.stx_atime.tv_nsec);
    out->changed_ns = nanoseconds(x.stx_ctime.tv_sec, x.stx_ctime.tv_nsec);
    out->created_ns = (x.stx_mask & STATX_BTIME) ? nanoseconds(x.stx_btime.tv_sec, x.stx_btime.tv_nsec) : 0;
    out->identity = x.stx_ino; out->device = makedev(x.stx_dev_major, x.stx_dev_minor);
    return DOTNET_PAL_OK;
}
static uint32_t file_open(const uint8_t *path, size_t length, uint32_t flags, uint32_t mode, void **out) {
    if (pal_files_fault == 2) { *out = NULL; return DOTNET_PAL_OK; } /* success without a handle */
    char name[PATH_MAX]; if (!terminated(name, path, length)) return DOTNET_PAL_NAME_TOO_LONG;
    int native = O_CLOEXEC | ((flags & 3u) == 3u ? O_RDWR : flags & DOTNET_PAL_FILE_WRITE ? O_WRONLY : O_RDONLY);
    if (flags & DOTNET_PAL_FILE_CREATE) native |= O_CREAT;
    if (flags & DOTNET_PAL_FILE_EXCLUSIVE) native |= O_EXCL;
    if (flags & DOTNET_PAL_FILE_TRUNCATE) native |= O_TRUNC;
    int fd; do fd = open(name, native, (mode_t)mode); while (fd < 0 && errno == EINTR);
    if (fd < 0) return failure();
    struct stat st;
    if (fstat(fd, &st) != 0) { uint32_t status = failure(); close(fd); return status; }
    if (S_ISDIR(st.st_mode)) { close(fd); return DOTNET_PAL_IS_DIRECTORY; }
    *out = (void*)(intptr_t)(fd + 1); return DOTNET_PAL_OK;
}
/* A transfer the descriptor was not opened for: EBADF (EINVAL from ftruncate) on a descriptor that is alive. */
static uint32_t refused(void *file) {
    int error = errno;
    if ((error == EBADF || error == EINVAL) && fcntl(descriptor(file), F_GETFL) >= 0) return DOTNET_PAL_ACCESS_DENIED;
    errno = error; return failure();
}
static uint32_t file_close(void *file) { return close(descriptor(file)) == 0 || errno == EINTR ? DOTNET_PAL_OK : failure(); }
static uint32_t file_read_at(void *file, uint64_t offset, uint8_t *data, size_t capacity, size_t *got) {
    if (pal_files_fault == 2) { *got = capacity + 1; return DOTNET_PAL_OK; } /* more than the buffer holds */
    for (;;) {
        ssize_t n = pread(descriptor(file), data, capacity, (off_t)offset);
        if (n < 0 && errno == EINTR) continue;
        if (n < 0) return refused(file);
        *got = (size_t)n; return DOTNET_PAL_OK;
    }
}
static uint32_t file_write_at(void *file, uint64_t offset, const uint8_t *data, size_t size, size_t *written) {
    if (pal_files_fault == 2) { *written = 0; return DOTNET_PAL_OK; } /* success without progress */
    for (;;) {
        ssize_t n = pwrite(descriptor(file), data, size, (off_t)offset);
        if (n < 0 && errno == EINTR) continue;
        if (n < 0) return refused(file);
        if (n == 0) return DOTNET_PAL_OS_ERROR;
        *written = (size_t)n; return DOTNET_PAL_OK;
    }
}
static uint32_t file_set_size(void *file, uint64_t size) {
    if (pal_files_fault == 2) return 99u; /* no such status */
    int rc; do rc = ftruncate(descriptor(file), (off_t)size); while (rc != 0 && errno == EINTR);
    return rc == 0 ? DOTNET_PAL_OK : refused(file);
}
static uint32_t file_flush(void *file) {
    if (pal_files_fault == 2) return DOTNET_PAL_BUFFER_TOO_SMALL; /* a status this call does not have */
    int rc; do rc = fsync(descriptor(file)); while (rc != 0 && errno == EINTR);
    return result(rc);
}
static uint32_t file_status(void *file, dotnet_pal_file_status *out, size_t size) {
    if (size < sizeof *out) return DOTNET_PAL_INVALID_ARGUMENT;
    if (pal_files_fault == 2) { memset(out, 0, sizeof *out); out->mode = 0644; out->size = 5; return DOTNET_PAL_OK; } /* no node kind */
    return describe(descriptor(file), "", AT_EMPTY_PATH, out);
}
static uint32_t file_path_status(const uint8_t *path, size_t length, uint32_t follow, dotnet_pal_file_status *out, size_t size) {
    if (size < sizeof *out) return DOTNET_PAL_INVALID_ARGUMENT;
    if (pal_files_fault == 2) { memset(out, 0, sizeof *out); out->kind = DOTNET_PAL_NODE_FILE; out->mode = 0100644; return DOTNET_PAL_OK; } /* file type bits in mode */
    char name[PATH_MAX]; if (!terminated(name, path, length)) return DOTNET_PAL_NAME_TOO_LONG;
    return describe(AT_FDCWD, name, follow ? 0 : AT_SYMLINK_NOFOLLOW, out);
}
static uint32_t file_remove(const uint8_t *path, size_t length) {
    if (pal_files_fault == 2) return DOTNET_PAL_WOULD_BLOCK; /* a status of the sockets group */
    char name[PATH_MAX]; if (!terminated(name, path, length)) return DOTNET_PAL_NAME_TOO_LONG;
    return result(unlink(name));
}
static uint32_t file_rename(const uint8_t *from, size_t from_length, const uint8_t *to, size_t to_length) {
    if (pal_files_fault == 2) return DOTNET_PAL_TIMEOUT; /* a status of the kernel group */
    char source[PATH_MAX], target[PATH_MAX];
    if (!terminated(source, from, from_length) || !terminated(target, to, to_length)) return DOTNET_PAL_NAME_TOO_LONG;
    return result(rename(source, target));
}
static uint32_t directory_create(const uint8_t *path, size_t length, uint32_t mode) {
    char name[PATH_MAX]; if (!terminated(name, path, length)) return DOTNET_PAL_NAME_TOO_LONG;
    return result(mkdir(name, (mode_t)mode));
}
static uint32_t directory_remove(const uint8_t *path, size_t length) {
    char name[PATH_MAX]; if (!terminated(name, path, length)) return DOTNET_PAL_NAME_TOO_LONG;
    if (rmdir(name) == 0) return DOTNET_PAL_OK;
    return errno == EEXIST ? DOTNET_PAL_NOT_EMPTY : failure();
}
static uint32_t directory_open(const uint8_t *path, size_t length, void **out) {
    if (pal_files_fault == 2) { *out = NULL; return DOTNET_PAL_OK; } /* success without a handle */
    char name[PATH_MAX]; if (!terminated(name, path, length)) return DOTNET_PAL_NAME_TOO_LONG;
    int fd = open(name, O_RDONLY | O_DIRECTORY | O_CLOEXEC);
    if (fd < 0) return failure();
    DIR *stream = fdopendir(fd);
    if (!stream) { uint32_t status = failure(); close(fd); return status; }
    *out = stream; return DOTNET_PAL_OK;
}
static uint32_t entry(uint8_t *name, size_t *length, uint32_t *kind, const char *text, size_t size, uint32_t node_kind, uint32_t status) {
    memcpy(name, text, size); *length = size; *kind = node_kind; return status;
}
static uint32_t directory_read(void *directory, uint8_t *name, size_t capacity, size_t *length, uint32_t *kind) {
    if (pal_files_fault == 2) {
        static int step;
        switch (step++) {
        case 0: return entry(name, length, kind, ".", 1, DOTNET_PAL_NODE_DIRECTORY, DOTNET_PAL_OK);  /* skipped, */
        case 1: return entry(name, length, kind, "..", 2, DOTNET_PAL_NODE_DIRECTORY, DOTNET_PAL_OK); /* skipped, */
        case 2: return entry(name, length, kind, "kept", 4, DOTNET_PAL_NODE_FILE, DOTNET_PAL_OK);    /* and the first real entry is delivered */
        case 3: return entry(name, length, kind, "a/b", 3, DOTNET_PAL_NODE_FILE, DOTNET_PAL_OK);     /* a path, not a name */
        case 4: return entry(name, length, kind, "a\0b", 3, DOTNET_PAL_NODE_FILE, DOTNET_PAL_OK);    /* embedded NUL */
        case 5: return entry(name, length, kind, "kind", 4, 0, DOTNET_PAL_OK);                       /* no node kind */
        case 6: return entry(name, length, kind, "kind", 4, 5, DOTNET_PAL_OK);                       /* unknown node kind */
        case 7: return entry(name, length, kind, "", 0, DOTNET_PAL_NODE_FILE, DOTNET_PAL_OK);        /* empty name */
        case 8: memset(name, 'x', DOTNET_PAL_MAX_ENTRY_NAME); *length = DOTNET_PAL_MAX_ENTRY_NAME + 1; *kind = DOTNET_PAL_NODE_FILE; return DOTNET_PAL_OK; /* longer than any name */
        default: return entry(name, length, kind, "junk", 4, DOTNET_PAL_NODE_FILE, DOTNET_PAL_ACCESS_DENIED); /* outputs written by a failing call */
        }
    }
    struct dirent *next;
    do { errno = 0; next = readdir(directory); } while (next && (strcmp(next->d_name, ".") == 0 || strcmp(next->d_name, "..") == 0));
    if (!next) return errno == 0 ? DOTNET_PAL_NOT_FOUND : failure();
    size_t size = strlen(next->d_name);
    if (size > capacity) return DOTNET_PAL_OS_ERROR;
    struct stat st; /* a lookup instead of d_type: the second opinion on what the Linux provider reads */
    if (fstatat(dirfd(directory), next->d_name, &st, AT_SYMLINK_NOFOLLOW) != 0) return failure();
    memcpy(name, next->d_name, size); *length = size; *kind = node(st.st_mode);
    return DOTNET_PAL_OK;
}
static uint32_t directory_close(void *directory) { return closedir(directory) == 0 || errno == EINTR ? DOTNET_PAL_OK : failure(); }
static uint32_t current_directory(uint8_t *out, size_t capacity, size_t *needed) {
    if (pal_files_fault == 2) {
        static int step;
        switch (step++) {
        case 0: if (capacity >= 4) memcpy(out, "/tmp", 4); *needed = 4; return DOTNET_PAL_OK; /* no terminator */
        case 1: if (capacity >= 4) memcpy(out, "/t\0", 4); *needed = 4; return DOTNET_PAL_OK;  /* terminator inside the text */
        case 2: if (capacity >= 1) out[0] = 0; *needed = 1; return DOTNET_PAL_OK;              /* empty path */
        default: if (capacity >= 2) memcpy(out, "/", 2); *needed = DOTNET_PAL_MAX_NAME + 2; return DOTNET_PAL_BUFFER_TOO_SMALL; /* longer than any path */
        }
    }
    char buffer[PATH_MAX];
    if (!getcwd(buffer, sizeof buffer)) return errno == ERANGE ? DOTNET_PAL_NAME_TOO_LONG : failure();
    *needed = strlen(buffer) + 1;
    if (*needed > capacity) return DOTNET_PAL_BUFFER_TOO_SMALL;
    memcpy(out, buffer, *needed); return DOTNET_PAL_OK;
}
static uint32_t file_set_mode(const uint8_t *path, size_t length, uint32_t mode) {
    if (pal_files_fault == 2) return DOTNET_PAL_WOULD_BLOCK; /* a status only the lock calls have */
    char name[PATH_MAX]; if (!terminated(name, path, length)) return DOTNET_PAL_NAME_TOO_LONG;
    return result(fchmodat(AT_FDCWD, name, (mode_t)mode, 0));
}
static uint32_t file_set_file_mode(void *file, uint32_t mode) {
    if (pal_files_fault == 2) return 99u; /* no such status */
    return result(fchmod(descriptor(file), (mode_t)mode));
}
/* A time for utimensat: DOTNET_PAL_TIME_KEEP is the one the node keeps. */
static struct timespec moment(uint64_t ns) {
    struct timespec time = {0, UTIME_OMIT};
    if (ns != DOTNET_PAL_TIME_KEEP) { time.tv_sec = (time_t)(ns / UINT64_C(1000000000)); time.tv_nsec = (long)(ns % UINT64_C(1000000000)); }
    return time;
}
static uint32_t file_set_times(const uint8_t *path, size_t length, uint32_t follow, uint64_t accessed_ns, uint64_t modified_ns) {
    if (pal_files_fault == 2) return DOTNET_PAL_BUFFER_TOO_SMALL; /* a status of the calls that return text */
    char name[PATH_MAX]; if (!terminated(name, path, length)) return DOTNET_PAL_NAME_TOO_LONG;
    const struct timespec times[2] = {moment(accessed_ns), moment(modified_ns)};
    return result(utimensat(AT_FDCWD, name, times, follow ? 0 : AT_SYMLINK_NOFOLLOW));
}
static uint32_t file_set_file_times(void *file, uint64_t accessed_ns, uint64_t modified_ns) {
    if (pal_files_fault == 2) return DOTNET_PAL_TIMEOUT; /* a status of the kernel group */
    const struct timespec times[2] = {moment(accessed_ns), moment(modified_ns)};
    return result(futimens(descriptor(file), times));
}
static uint32_t file_link(const uint8_t *existing, size_t existing_length, const uint8_t *created, size_t created_length) {
    if (pal_files_fault == 2) return DOTNET_PAL_WOULD_BLOCK;
    char source[PATH_MAX], target[PATH_MAX];
    if (!terminated(source, existing, existing_length) || !terminated(target, created, created_length)) return DOTNET_PAL_NAME_TOO_LONG;
    return result(linkat(AT_FDCWD, source, AT_FDCWD, target, 0));
}
static uint32_t file_symlink(const uint8_t *target, size_t target_length, const uint8_t *created, size_t created_length) {
    if (pal_files_fault == 2) return DOTNET_PAL_IN_PROGRESS; /* a status of the sockets group */
    char text[PATH_MAX], name[PATH_MAX]; /* the target is text to store, not a path to look at */
    if (!terminated(text, target, target_length) || !terminated(name, created, created_length)) return DOTNET_PAL_NAME_TOO_LONG;
    return result(symlinkat(text, AT_FDCWD, name));
}
static uint32_t text_result(const char *text, size_t size, uint8_t *out, size_t capacity, size_t *needed) {
    if (size > DOTNET_PAL_MAX_NAME) return DOTNET_PAL_NAME_TOO_LONG;
    *needed = size + 1;
    if (*needed > capacity) return DOTNET_PAL_BUFFER_TOO_SMALL;
    memcpy(out, text, size); out[size] = 0; return DOTNET_PAL_OK;
}
/* The text contract of read_link and real_path, broken another way on every call. */
static uint32_t broken_text(int step, uint8_t *out, size_t capacity, size_t *needed) {
    switch (step) {
    case 0: if (capacity >= 4) memcpy(out, "/tmp", 4); *needed = 4; return DOTNET_PAL_OK; /* no terminator */
    case 1: if (capacity >= 4) memcpy(out, "/t\0", 4); *needed = 4; return DOTNET_PAL_OK;  /* terminator inside the text */
    case 2: if (capacity >= 2) memcpy(out, "/", 2); *needed = 0; return DOTNET_PAL_OK;     /* no length */
    case 3: if (capacity >= 1) out[0] = 0; *needed = 1; return DOTNET_PAL_OK;              /* empty text */
    case 4: if (capacity >= 2) memcpy(out, "/", 2); *needed = DOTNET_PAL_MAX_NAME + 2; return DOTNET_PAL_BUFFER_TOO_SMALL; /* longer than any text */
    default: *needed = 4; return DOTNET_PAL_BUFFER_TOO_SMALL; /* too small, though it fits: nothing written */
    }
}
static uint32_t file_read_link(const uint8_t *path, size_t length, uint8_t *out, size_t capacity, size_t *needed) {
    static int step;
    if (pal_files_fault == 2) return broken_text(step++, out, capacity, needed);
    char name[PATH_MAX], text[PATH_MAX + 1]; if (!terminated(name, path, length)) return DOTNET_PAL_NAME_TOO_LONG;
    ssize_t size = readlinkat(AT_FDCWD, name, text, sizeof text);
    return size < 0 ? failure() : text_result(text, (size_t)size, out, capacity, needed);
}
static uint32_t file_real_path(const uint8_t *path, size_t length, uint8_t *out, size_t capacity, size_t *needed) {
    static int step;
    if (pal_files_fault == 2) return broken_text(step++, out, capacity, needed);
    char name[PATH_MAX]; if (!terminated(name, path, length)) return DOTNET_PAL_NAME_TOO_LONG;
    char *resolved = realpath(name, NULL); /* the C library's allocation: the second opinion on a fixed buffer */
    if (!resolved) return failure();
    uint32_t status = text_result(resolved, strlen(resolved), out, capacity, needed);
    free(resolved); return status;
}
static uint32_t file_set_current_directory(const uint8_t *path, size_t length) {
    if (pal_files_fault == 2) return DOTNET_PAL_WOULD_BLOCK;
    char name[PATH_MAX]; if (!terminated(name, path, length)) return DOTNET_PAL_NAME_TOO_LONG;
    return result(chdir(name));
}
static uint32_t file_lock(void *file, uint32_t mode, uint32_t wait) {
    if (pal_files_fault == 2) return DOTNET_PAL_BUFFER_TOO_SMALL; /* WOULD_BLOCK is this call's own; this one is not */
    int operation = (mode == DOTNET_PAL_LOCK_SHARED ? LOCK_SH : mode == DOTNET_PAL_LOCK_EXCLUSIVE ? LOCK_EX : LOCK_UN) | (wait ? 0 : LOCK_NB);
    int rc; do rc = flock(descriptor(file), operation); while (rc != 0 && errno == EINTR);
    return rc != 0 && errno == EWOULDBLOCK ? DOTNET_PAL_WOULD_BLOCK : result(rc);
}
/* An open-file-description lock: held by the descriptor behind the handle, not by the process. */
static uint32_t file_lock_range(void *file, uint64_t offset, uint64_t length, uint32_t mode) {
    if (pal_files_fault == 2) return 99u;
    struct flock range = {.l_type = mode == DOTNET_PAL_LOCK_SHARED ? F_RDLCK : mode == DOTNET_PAL_LOCK_EXCLUSIVE ? F_WRLCK : F_UNLCK,
                          .l_whence = SEEK_SET, .l_start = (off_t)offset, .l_len = (off_t)length};
    int rc; do rc = fcntl(descriptor(file), F_OFD_SETLK, &range); while (rc != 0 && errno == EINTR);
    if (rc == 0) return DOTNET_PAL_OK;
    if (errno == EAGAIN || errno == EACCES) return DOTNET_PAL_WOULD_BLOCK;
    return errno == EBADF ? refused(file) : failure(); /* a read lock on a write-only descriptor, or the reverse */
}
static const dotnet_pal_host_files table = {
    {DOTNET_PAL_ABI_VERSION, sizeof(dotnet_pal_host_files), DOTNET_PAL_CAP_FILES},
    {file_open, file_close, file_read_at, file_write_at, file_set_size, file_flush, file_status, file_path_status, file_remove, file_rename,
     directory_create, directory_remove, directory_open, directory_read, directory_close, current_directory, NULL,
     file_set_mode, file_set_file_mode, file_set_times, file_set_file_times, file_link, file_symlink, file_read_link, file_real_path,
     file_set_current_directory, file_lock, file_lock_range},
};
static const dotnet_pal_host_files malformed = {
    {DOTNET_PAL_ABI_VERSION, sizeof(dotnet_pal_host_files), DOTNET_PAL_CAP_FILES},
    {file_open, file_close, file_read_at, file_write_at, file_set_size, file_flush, file_status, file_path_status, file_remove, NULL,
     directory_create, directory_remove, directory_open, directory_read, directory_close, current_directory, NULL,
     file_set_mode, file_set_file_mode, file_set_times, file_set_file_times, file_link, file_symlink, file_read_link, file_real_path,
     file_set_current_directory, file_lock, file_lock_range},
};
static const dotnet_pal_host_files minimal = {
    {DOTNET_PAL_ABI_VERSION, sizeof(dotnet_pal_host_files), DOTNET_PAL_CAP_FILES},
    {file_open, file_close, file_read_at, file_write_at, file_set_size, file_flush, file_status, file_path_status, file_remove, file_rename,
     directory_create, directory_remove, directory_open, directory_read, directory_close, current_directory, NULL,
     NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL},
};
const dotnet_pal_host_files *dotnet_pal_host_files_v2(void) { return pal_files_fault == 1 ? &malformed : pal_files_fault == 3 ? &minimal : &table; }

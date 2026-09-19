/* System.Native over the boundary: the descriptor table, the standard streams,
 * files and directories. Files go through the boundary's files group, which has
 * no cursor, no descriptors and no errno: this unit keeps the POSIX face managed
 * code expects (small integer descriptors, a shared position per open file,
 * errno) on top of it. Without the files group nothing exists: opening reports
 * ENOTSUP and path queries ENOENT, as before the group existed.
 *
 * Permission and timestamp changes, links, the working directory change and
 * advisory locks are optional operations of the group: a provider without one
 * answers UNSUPPORTED, which arrives here as ENOTSUP. Memory-mapped files and
 * file system notifications are not part of the boundary. */
#include "system_native_internal.h"

int sn_errno(uint32_t status) {
    switch (status) {
        case DOTNET_PAL_OK: return 0;
        case DOTNET_PAL_UNSUPPORTED: return ENOTSUP; case DOTNET_PAL_INVALID_ARGUMENT: return EINVAL; case DOTNET_PAL_OUT_OF_MEMORY: return ENOMEM;
        case DOTNET_PAL_TIMEOUT: return ETIMEDOUT; case DOTNET_PAL_BUSY: return EBUSY; case DOTNET_PAL_BUFFER_TOO_SMALL: return ERANGE;
        case DOTNET_PAL_NOT_FOUND: return ENOENT; case DOTNET_PAL_ALREADY_EXISTS: return EEXIST; case DOTNET_PAL_ACCESS_DENIED: return EACCES;
        case DOTNET_PAL_IS_DIRECTORY: return EISDIR; case DOTNET_PAL_NOT_DIRECTORY: return ENOTDIR; case DOTNET_PAL_NOT_EMPTY: return ENOTEMPTY;
        case DOTNET_PAL_NO_SPACE: return ENOSPC; case DOTNET_PAL_WOULD_BLOCK: return EAGAIN; case DOTNET_PAL_BROKEN_PIPE: return EPIPE;
        case DOTNET_PAL_CONNECTION_REFUSED: return ECONNREFUSED; case DOTNET_PAL_CONNECTION_RESET: return ECONNRESET;
        case DOTNET_PAL_CONNECTION_ABORTED: return ECONNABORTED; case DOTNET_PAL_NOT_CONNECTED: return ENOTCONN;
        case DOTNET_PAL_ALREADY_CONNECTED: return EISCONN; case DOTNET_PAL_ADDRESS_IN_USE: return EADDRINUSE;
        case DOTNET_PAL_ADDRESS_NOT_AVAILABLE: return EADDRNOTAVAIL; case DOTNET_PAL_NETWORK_UNREACHABLE: return ENETUNREACH;
        case DOTNET_PAL_HOST_UNREACHABLE: return EHOSTUNREACH; case DOTNET_PAL_IN_PROGRESS: return EINPROGRESS;
        case DOTNET_PAL_TOO_MANY_HANDLES: return EMFILE; case DOTNET_PAL_NAME_TOO_LONG: return ENAMETOOLONG; case DOTNET_PAL_READ_ONLY: return EROFS;
        case DOTNET_PAL_CROSS_DEVICE: return EXDEV; case DOTNET_PAL_MESSAGE_TOO_LARGE: return EMSGSIZE;
        default: return EIO;
    }
}

/* ---- the descriptor table -------------------------------------------------- */
/* Critical sections are a few loads and stores and never call the boundary, so a
 * spin lock is enough, and on a cooperative port it is never even contended. */
static int32_t table_lock;
void sn_lock(void) {
    while (__atomic_exchange_n(&table_lock, 1, __ATOMIC_ACQUIRE)) {
        const dotnet_pal_api *a = sn_api();
        if (sn_has(a, DOTNET_PAL_SERVICES_API_SIZE, DOTNET_PAL_CAP_SCHEDULER) && a->services.yield_thread) (void)a->services.yield_thread();
    }
}
void sn_unlock(void) { __atomic_store_n(&table_lock, 0, __ATOMIC_RELEASE); }

/* The three standard streams exist from the start and are never destroyed. */
static sn_object standard[3] = {
    {.kind = SN_STREAM, .references = 1, .descriptors = 1, .stream = 0},
    {.kind = SN_STREAM, .references = 1, .descriptors = 1, .stream = 1},
    {.kind = SN_STREAM, .references = 1, .descriptors = 1, .stream = 2},
};
static sn_object *table[SN_MAX_FD] = {&standard[0], &standard[1], &standard[2]};

sn_object *sn_new(sn_kind kind, void *handle) {
    sn_object *object = SystemNative_Calloc(1, sizeof *object);
    if (!object) { errno = ENOMEM; return NULL; }
    object->kind = kind; object->handle = handle; object->references = 1; object->descriptors = 1;
    return object;
}
static void watch_destroy(sn_object *object);
/* Outside the lock: closing a handle is a boundary call. */
static void destroy(sn_object *object) {
    if (object->kind == SN_STREAM) return;
    if (object->kind == SN_FILE && object->handle) { const dotnet_pal_files_ops *f = sn_files(); if (f) (void)f->close(object->handle); }
    if (object->kind == SN_SOCKET && object->handle) { const dotnet_pal_sockets_ops *s = sn_sockets(); if (s) (void)s->close(object->handle); }
    if (object->kind == SN_PORT && object->self) sn_port_destroy(object->self);
    if (object->kind == SN_PIPE) { const dotnet_pal_processes_ops *c = sn_processes(); sn_pipe_destroy(object); if (c && object->handle) (void)c->pipe_close(object->handle); }
    if (object->kind == SN_WATCH) watch_destroy(object);
    SystemNative_Free(object);
}
sn_object *sn_pin(intptr_t fd, sn_kind kind, int wrong_kind) {
    if (fd < 0 || fd >= SN_MAX_FD) { errno = EBADF; return NULL; }
    sn_lock();
    sn_object *object = table[fd];
    if (object && (kind == 0 || object->kind == kind)) object->references++;
    else { errno = object ? wrong_kind : EBADF; object = NULL; }
    sn_unlock();
    return object;
}
void sn_unpin(sn_object *object) {
    sn_lock();
    bool last = --object->references == 0;
    sn_unlock();
    if (last) destroy(object);
}
static intptr_t install_locked(sn_object *object) {
    for (intptr_t fd = 3; fd < SN_MAX_FD; ++fd) if (!table[fd]) { table[fd] = object; return fd; }
    return -1;
}
intptr_t sn_install(sn_object *object) {
    sn_lock();
    intptr_t fd = install_locked(object);
    sn_unlock();
    if (fd < 0) { destroy(object); return sn_fail(EMFILE); }
    return fd;
}
PALEXPORT intptr_t SystemNative_Dup(intptr_t oldfd) {
    if (oldfd < 0 || oldfd >= SN_MAX_FD) return sn_fail(EBADF);
    sn_lock();
    sn_object *object = table[oldfd];
    intptr_t fd = object ? install_locked(object) : -1;
    if (fd >= 0) { object->references++; object->descriptors++; }
    sn_unlock();
    return fd >= 0 ? fd : sn_fail(object ? EMFILE : EBADF);
}
PALEXPORT int32_t SystemNative_Close(intptr_t fd) {
    if (fd < 0 || fd >= SN_MAX_FD) return sn_fail(EBADF);
    sn_lock();
    sn_object *object = table[fd];
    /* The standard streams stay open: the runtime and the port keep writing to them. */
    if (!object || fd < 3) { sn_unlock(); return object ? 0 : sn_fail(EBADF); }
    table[fd] = NULL;
    bool closed = --object->descriptors == 0, last = --object->references == 0;
    struct sn_port *port = closed && object->port ? sn_socket_closing(object) : NULL;
    if (closed && object->kind == SN_PIPE) sn_pipe_closing(object);
    sn_unlock();
    /* A waiter that snapshotted this socket still pins it; waking it lets the close finish. */
    if (port) sn_port_wake(port);
    if (last) destroy(object);
    return 0;
}
PALEXPORT int32_t SystemNative_FcntlGetFD(intptr_t fd) {
    sn_object *object = sn_pin(fd, 0, 0);
    if (!object) return -1;
    /* Nothing here is ever inherited by another process: everything but the streams is close-on-exec. */
    int32_t flags = object->kind == SN_STREAM ? 0 : 1;
    sn_unpin(object);
    return flags;
}
PALEXPORT int32_t SystemNative_FcntlSetFD(intptr_t fd, int32_t flags) {
    (void)flags;
    sn_object *object = sn_pin(fd, 0, 0);
    if (!object) return -1;
    sn_unpin(object);
    return 0;
}

/* ---- transfers --------------------------------------------------------------- */
static int32_t clamp(size_t count) { return count > INT32_MAX ? INT32_MAX : (int32_t)count; }
static int32_t stream_write(const sn_object *object, const void *buffer, int32_t size) {
    const dotnet_pal_streams_ops *s = sn_streams(); size_t written = 0;
    if (object->stream == 0) return sn_fail(EBADF);
    if (!s) return sn_fail(ENOTSUP);
    uint32_t status = s->write((uint32_t)object->stream, buffer, (size_t)size, &written);
    return status == DOTNET_PAL_OK ? clamp(written) : sn_fail(status == DOTNET_PAL_UNSUPPORTED ? ENOTSUP : EIO);
}
static int32_t stream_read(const sn_object *object, void *buffer, int32_t size) {
    const dotnet_pal_streams_ops *s = sn_streams(); size_t got = 0;
    if (object->stream != 0) return sn_fail(EBADF);
    if (!s) return sn_fail(ENOTSUP);
    uint32_t status = s->read(0, buffer, (size_t)size, &got);
    return status == DOTNET_PAL_OK ? clamp(got) : sn_fail(status == DOTNET_PAL_UNSUPPORTED ? ENOTSUP : EIO);
}
static bool readable(const sn_object *object) { return (object->open_flags & PAL_O_ACCESS_MODE_MASK) != PAL_O_WRONLY; }
static bool writable(const sn_object *object) { return (object->open_flags & PAL_O_ACCESS_MODE_MASK) != PAL_O_RDONLY; }
static int32_t file_read(sn_object *object, void *buffer, int32_t size, uint64_t offset) {
    const dotnet_pal_files_ops *f = sn_files(); size_t got = 0;
    if (!f) return sn_fail(EBADF);
    if (!readable(object)) return sn_fail(EBADF);
    uint32_t status = f->read_at(object->handle, offset, buffer, (size_t)size, &got);
    return status == DOTNET_PAL_OK ? clamp(got) : sn_fail(sn_errno(status));
}
/* All of the buffer or an error: managed callers of pwrite do loop, but O_SYNC and the cursor want one answer. */
static int32_t file_write(sn_object *object, const void *buffer, int32_t size, uint64_t offset) {
    const dotnet_pal_files_ops *f = sn_files(); size_t written = 0;
    if (!f) return sn_fail(EBADF);
    if (!writable(object)) return sn_fail(EBADF);
    uint32_t status = f->write_at(object->handle, offset, buffer, (size_t)size, &written);
    if (status != DOTNET_PAL_OK) return sn_fail(sn_errno(status));
    if ((object->open_flags & PAL_O_SYNC) && (status = f->flush(object->handle)) != DOTNET_PAL_OK) return sn_fail(sn_errno(status));
    return clamp(written);
}
/* A pipe to a child process moves bytes in the one direction it was made for. Writing always waits for room;
 * reading is the process unit's, because a read end can be switched to non-blocking. */
static int32_t pipe_transfer(sn_object *object, void *buffer, int32_t size, bool write) {
    const dotnet_pal_processes_ops *c = sn_processes(); size_t done = 0;
    if (!c || write != (object->open_flags == PAL_O_WRONLY)) return sn_fail(EBADF);
    if (!write) return sn_pipe_read(object, buffer, size);
    uint32_t status = c->pipe_write(object->handle, buffer, (size_t)size, &done);
    return status == DOTNET_PAL_OK ? clamp(done) : sn_fail(sn_errno(status));
}
/* The cursor moves by what was transferred. Two threads sharing one descriptor race for it, as they would with read(2) on separate cores. */
static uint64_t cursor(sn_object *object) { return __atomic_load_n(&object->position, __ATOMIC_ACQUIRE); }
static void advance(sn_object *object, int32_t by) { if (by > 0) (void)__atomic_add_fetch(&object->position, (uint64_t)by, __ATOMIC_ACQ_REL); }
static int32_t watch_read(sn_object *object, uint8_t *buffer, int32_t size);
PALEXPORT int32_t SystemNative_Write(intptr_t fd, const void* buffer, int32_t bufferSize) {
    if (bufferSize < 0) return sn_fail(EINVAL);
    sn_object *object = sn_pin(fd, 0, 0);
    if (!object) return -1;
    int32_t result;
    if (bufferSize == 0) result = 0;
    else if (object->kind == SN_STREAM) result = stream_write(object, buffer, bufferSize);
    else if (object->kind == SN_FILE) { result = file_write(object, buffer, bufferSize, cursor(object)); advance(object, result); }
    else if (object->kind == SN_PIPE) result = pipe_transfer(object, (void*)(uintptr_t)buffer, bufferSize, true);
    else result = sn_fail(object->kind == SN_SOCKET ? ENOTSUP : EBADF); /* sockets transfer through Send and Receive */
    sn_unpin(object);
    return result;
}
PALEXPORT int32_t SystemNative_Read(intptr_t fd, void* buffer, int32_t bufferSize) {
    if (bufferSize < 0) return sn_fail(EINVAL);
    sn_object *object = sn_pin(fd, 0, 0);
    if (!object) return -1;
    int32_t result;
    if (bufferSize == 0) result = 0;
    else if (object->kind == SN_STREAM) result = stream_read(object, buffer, bufferSize);
    else if (object->kind == SN_FILE) { result = file_read(object, buffer, bufferSize, cursor(object)); advance(object, result); }
    else if (object->kind == SN_PIPE) result = pipe_transfer(object, buffer, bufferSize, false);
    else if (object->kind == SN_WATCH) result = watch_read(object, buffer, bufferSize);
    else result = sn_fail(object->kind == SN_SOCKET ? ENOTSUP : EBADF);
    sn_unpin(object);
    return result;
}
PALEXPORT int32_t SystemNative_ReadStdin(void* buffer, int32_t bufferSize) { return SystemNative_Read(0, buffer, bufferSize); }
PALEXPORT int32_t SystemNative_IsATty(intptr_t fd) {
    sn_object *object = sn_pin(fd, 0, 0); uint32_t terminal = 0;
    if (!object) return 0;
    const dotnet_pal_streams_ops *s = sn_streams();
    if (object->kind == SN_STREAM && s && s->is_terminal((uint32_t)object->stream, &terminal) != DOTNET_PAL_OK) terminal = 0;
    sn_unpin(object);
    if (!terminal) errno = ENOTTY;
    return terminal ? 1 : 0;
}
PALEXPORT int32_t SystemNative_PRead(intptr_t fd, void* buffer, int32_t bufferSize, int64_t fileOffset) {
    if (bufferSize < 0 || fileOffset < 0) return sn_fail(EINVAL);
    sn_object *object = sn_pin(fd, 0, 0);
    if (!object) return -1;
    int32_t result = object->kind != SN_FILE ? sn_fail(ESPIPE) : bufferSize == 0 ? 0 : file_read(object, buffer, bufferSize, (uint64_t)fileOffset);
    sn_unpin(object);
    return result;
}
PALEXPORT int32_t SystemNative_PWrite(intptr_t fd, void* buffer, int32_t bufferSize, int64_t fileOffset) {
    if (bufferSize < 0 || fileOffset < 0) return sn_fail(EINVAL);
    sn_object *object = sn_pin(fd, 0, 0);
    if (!object) return -1;
    int32_t result = object->kind != SN_FILE ? sn_fail(ESPIPE) : bufferSize == 0 ? 0 : file_write(object, buffer, bufferSize, (uint64_t)fileOffset);
    sn_unpin(object);
    return result;
}
/* Vectored transfers stop at the first short or failed part, reporting what moved before it. */
static int64_t vectored(intptr_t fd, IOVector* vectors, int32_t count, int64_t offset, bool write) {
    if (count < 0 || offset < 0 || (count > 0 && !vectors)) return sn_fail(EINVAL);
    sn_object *object = sn_pin(fd, SN_FILE, ESPIPE);
    if (!object) return -1;
    int64_t total = 0;
    for (int32_t i = 0; i < count; ++i) {
        size_t left = vectors[i].Count; uint8_t *at = vectors[i].Base;
        while (left > 0) {
            int32_t part = left > INT32_MAX ? INT32_MAX : (int32_t)left;
            int32_t done = write ? file_write(object, at, part, (uint64_t)(offset + total)) : file_read(object, at, part, (uint64_t)(offset + total));
            if (done < 0) { sn_unpin(object); return total > 0 ? total : -1; }
            total += done; at += done; left -= (size_t)done;
            if (done < part) { sn_unpin(object); return total; }
        }
    }
    sn_unpin(object);
    return total;
}
PALEXPORT int64_t SystemNative_PReadV(intptr_t fd, IOVector* vectors, int32_t vectorCount, int64_t fileOffset) { return vectored(fd, vectors, vectorCount, fileOffset, false); }
PALEXPORT int64_t SystemNative_PWriteV(intptr_t fd, IOVector* vectors, int32_t vectorCount, int64_t fileOffset) { return vectored(fd, vectors, vectorCount, fileOffset, true); }

/* ---- status -------------------------------------------------------------------- */
static void fill_status(FileStatus *output, const dotnet_pal_file_status *status) {
    static const int32_t types[] = {0, PAL_S_IFREG, PAL_S_IFDIR, PAL_S_IFLNK, PAL_S_IFCHR};
    memset(output, 0, sizeof *output);
    output->Mode = types[status->kind <= DOTNET_PAL_NODE_OTHER ? status->kind : 0] | (int32_t)(status->mode & 07777);
    output->Size = status->size > INT64_MAX ? INT64_MAX : (int64_t)status->size;
    output->MTime = (int64_t)(status->modified_ns / 1000000000u); output->MTimeNsec = (int64_t)(status->modified_ns % 1000000000u);
    output->ATime = (int64_t)(status->accessed_ns / 1000000000u); output->ATimeNsec = (int64_t)(status->accessed_ns % 1000000000u);
    output->CTime = (int64_t)(status->changed_ns / 1000000000u); output->CTimeNsec = (int64_t)(status->changed_ns % 1000000000u);
    if (status->created_ns != 0) {
        output->Flags |= FILESTATUS_FLAGS_HAS_BIRTHTIME;
        output->BirthTime = (int64_t)(status->created_ns / 1000000000u); output->BirthTimeNsec = (int64_t)(status->created_ns % 1000000000u);
    }
    output->Ino = (int64_t)status->identity; output->Dev = (int64_t)status->device;
}
PALEXPORT int32_t SystemNative_FStat(intptr_t fd, FileStatus* output) {
    sn_object *object = sn_pin(fd, 0, 0);
    if (!object) return -1;
    int32_t result = 0;
    memset(output, 0, sizeof *output);
    if (object->kind == SN_FILE) {
        const dotnet_pal_files_ops *f = sn_files(); dotnet_pal_file_status status;
        uint32_t code = f ? f->status(object->handle, &status, sizeof status) : DOTNET_PAL_UNSUPPORTED;
        if (code == DOTNET_PAL_OK) fill_status(output, &status); else result = sn_fail(sn_errno(code));
    } else {
        /* A console stream is a character device, a socket a socket; neither is seekable. */
        output->Mode = object->kind == SN_STREAM ? (PAL_S_IFCHR | 0620) : object->kind == SN_SOCKET ? (PAL_S_IFSOCK | 0777) : (PAL_S_IFIFO | 0600);
    }
    sn_unpin(object);
    return result;
}
static int32_t path_status(const char *path, FileStatus *output, uint32_t follow) {
    const dotnet_pal_files_ops *f = sn_files(); dotnet_pal_file_status status;
    if (!path || !*path || !f) return sn_fail(ENOENT); /* no file system behind the boundary: no path exists */
    uint32_t code = f->path_status((const uint8_t*)path, strlen(path), follow, &status, sizeof status);
    if (code != DOTNET_PAL_OK) return sn_fail(sn_errno(code));
    fill_status(output, &status);
    return 0;
}
PALEXPORT int32_t SystemNative_Stat(const char* path, FileStatus* output) { return path_status(path, output, 1); }
PALEXPORT int32_t SystemNative_LStat(const char* path, FileStatus* output) { return path_status(path, output, 0); }
PALEXPORT int32_t SystemNative_Access(const char* path, int32_t mode) {
    /* Existence is what the boundary can answer; it has no notion of a caller identity to check permissions for. */
    FileStatus status; (void)mode;
    return path_status(path, &status, 1);
}

/* ---- files ------------------------------------------------------------------------ */
PALEXPORT intptr_t SystemNative_Open(const char* path, int32_t flags, int32_t mode) {
    const dotnet_pal_files_ops *f = sn_files();
    if (!f) return sn_fail(ENOTSUP);
    if (!path || !*path) return sn_fail(ENOENT);
    int32_t access = flags & PAL_O_ACCESS_MODE_MASK;
    if (access > PAL_O_RDWR || (flags & ~(PAL_O_ACCESS_MODE_MASK | PAL_O_CLOEXEC | PAL_O_CREAT | PAL_O_EXCL | PAL_O_TRUNC | PAL_O_SYNC | PAL_O_NOFOLLOW))) return sn_fail(EINVAL);
    uint32_t wanted = (access != PAL_O_WRONLY ? DOTNET_PAL_FILE_READ : 0) | (access != PAL_O_RDONLY ? DOTNET_PAL_FILE_WRITE : 0)
        | ((flags & PAL_O_CREAT) ? DOTNET_PAL_FILE_CREATE : 0) | ((flags & PAL_O_TRUNC) ? DOTNET_PAL_FILE_TRUNCATE : 0);
    if ((flags & PAL_O_EXCL) && (flags & PAL_O_CREAT)) wanted |= DOTNET_PAL_FILE_EXCLUSIVE;
    if ((flags & PAL_O_TRUNC) && access == PAL_O_RDONLY) return sn_fail(EINVAL);
    size_t length = strlen(path);
    if (flags & PAL_O_NOFOLLOW) {
        dotnet_pal_file_status status;
        if (f->path_status((const uint8_t*)path, length, 0, &status, sizeof status) == DOTNET_PAL_OK && status.kind == DOTNET_PAL_NODE_SYMLINK) return sn_fail(ELOOP);
    }
    void *handle = NULL;
    uint32_t code = f->open((const uint8_t*)path, length, wanted, (uint32_t)mode & 07777, &handle);
    if (code != DOTNET_PAL_OK) return sn_fail(sn_errno(code));
    sn_object *object = sn_new(SN_FILE, handle);
    if (!object) { (void)f->close(handle); return -1; }
    object->open_flags = flags;
    return sn_install(object);
}
PALEXPORT int64_t SystemNative_LSeek(intptr_t fd, int64_t offset, int32_t whence) {
    sn_object *object = sn_pin(fd, SN_FILE, ESPIPE);
    if (!object) return -1;
    int64_t base = 0, result;
    if (whence == PAL_SEEK_CUR) base = (int64_t)cursor(object);
    else if (whence == PAL_SEEK_END) {
        const dotnet_pal_files_ops *f = sn_files(); dotnet_pal_file_status status;
        uint32_t code = f ? f->status(object->handle, &status, sizeof status) : DOTNET_PAL_UNSUPPORTED;
        if (code != DOTNET_PAL_OK) { sn_unpin(object); return sn_fail(sn_errno(code)); }
        base = (int64_t)status.size;
    } else if (whence != PAL_SEEK_SET) { sn_unpin(object); return sn_fail(EINVAL); }
    if (__builtin_add_overflow(base, offset, &result) || result < 0) { sn_unpin(object); return sn_fail(EINVAL); }
    __atomic_store_n(&object->position, (uint64_t)result, __ATOMIC_RELEASE);
    sn_unpin(object);
    return result;
}
PALEXPORT int32_t SystemNative_FTruncate(intptr_t fd, int64_t length) {
    if (length < 0) return sn_fail(EINVAL);
    sn_object *object = sn_pin(fd, SN_FILE, EINVAL);
    if (!object) return -1;
    const dotnet_pal_files_ops *f = sn_files();
    int32_t result = !writable(object) ? sn_fail(EINVAL) : sn_status(f ? f->set_size(object->handle, (uint64_t)length) : DOTNET_PAL_UNSUPPORTED);
    sn_unpin(object);
    return result;
}
PALEXPORT int32_t SystemNative_FSync(intptr_t fd) {
    sn_object *object = sn_pin(fd, 0, 0);
    if (!object) return -1;
    const dotnet_pal_files_ops *f = sn_files();
    int32_t result = object->kind != SN_FILE ? 0 : sn_status(f ? f->flush(object->handle) : DOTNET_PAL_UNSUPPORTED);
    sn_unpin(object);
    return result;
}
/* Advisory locks are an optional operation of the files group. A provider without them answers ENOTSUP,
 * which the BCL treats like every failure but EWOULDBLOCK: "no locking here". */
PALEXPORT int32_t SystemNative_FLock(intptr_t fd, int32_t operation) {
    int32_t what = operation & ~PAL_LOCK_NB;
    uint32_t mode = what == PAL_LOCK_SH ? DOTNET_PAL_LOCK_SHARED : what == PAL_LOCK_EX ? DOTNET_PAL_LOCK_EXCLUSIVE : what == PAL_LOCK_UN ? DOTNET_PAL_LOCK_UNLOCK : 0;
    if (mode == 0) return sn_fail(EINVAL);
    sn_object *object = sn_pin(fd, 0, 0);
    if (!object) return -1;
    const dotnet_pal_files_ops *f = sn_files();
    int32_t result = object->kind != SN_FILE ? 0 : sn_status(f && f->lock ? f->lock(object->handle, mode, (operation & PAL_LOCK_NB) ? 0 : 1) : DOTNET_PAL_UNSUPPORTED);
    sn_unpin(object);
    return result;
}
/* lockType is Interop.Sys.LockType: 0 read, 1 write, anything else unlock. A length of zero means "to the end of the file". */
PALEXPORT int32_t SystemNative_LockFileRegion(intptr_t fd, int64_t offset, int64_t length, int16_t lockType) {
    if (offset < 0 || length < 0) return sn_fail(EINVAL);
    sn_object *object = sn_pin(fd, SN_FILE, EINVAL);
    if (!object) return -1;
    const dotnet_pal_files_ops *f = sn_files();
    uint32_t mode = lockType == 0 ? DOTNET_PAL_LOCK_SHARED : lockType == 1 ? DOTNET_PAL_LOCK_EXCLUSIVE : DOTNET_PAL_LOCK_UNLOCK;
    uint64_t span = length == 0 ? (uint64_t)(INT64_MAX - offset) : (uint64_t)length;
    int32_t result = sn_status(f && f->lock_range ? f->lock_range(object->handle, (uint64_t)offset, span, mode) : DOTNET_PAL_UNSUPPORTED);
    sn_unpin(object);
    return result;
}
PALEXPORT int32_t SystemNative_PosixFAdvise(intptr_t fd, int64_t offset, int64_t length, int32_t advice) {
    (void)offset; (void)length; (void)advice;
    sn_object *object = sn_pin(fd, 0, 0);
    if (!object) return EBADF; /* this entry point returns the error instead of setting errno */
    sn_unpin(object);
    return 0;
}
PALEXPORT int32_t SystemNative_FAllocate(intptr_t fd, int64_t offset, int64_t length) {
    (void)offset; (void)length;
    sn_object *object = sn_pin(fd, 0, 0);
    if (!object) return -1;
    sn_unpin(object);
    return sn_fail(ENOTSUP); /* reserving space without changing the size is not part of the boundary */
}
PALEXPORT uint32_t SystemNative_GetFileSystemType(intptr_t fd) {
    sn_object *object = sn_pin(fd, 0, 0);
    if (object) { sn_unpin(object); errno = ENOTSUP; }
    return 0;
}
PALEXPORT int32_t SystemNative_CopyFile(intptr_t sourceFd, intptr_t destinationFd, int64_t sourceLength) {
    (void)sourceLength;
    sn_object *source = sn_pin(sourceFd, SN_FILE, EINVAL);
    if (!source) return -1;
    sn_object *destination = sn_pin(destinationFd, SN_FILE, EINVAL);
    if (!destination) { sn_unpin(source); return -1; }
    static const int32_t CHUNK = 64 * 1024;
    uint8_t *buffer = SystemNative_Malloc((uintptr_t)CHUNK);
    int32_t result = buffer ? 0 : sn_fail(ENOMEM);
    while (result == 0) {
        int32_t got = file_read(source, buffer, CHUNK, cursor(source));
        if (got <= 0) { result = got; break; }
        advance(source, got);
        for (int32_t done = 0; done < got && result == 0;) {
            int32_t put = file_write(destination, buffer + done, got - done, cursor(destination));
            if (put <= 0) result = put < 0 ? -1 : sn_fail(EIO); else { advance(destination, put); done += put; }
        }
    }
    SystemNative_Free(buffer);
    /* Then the times and the permission bits of the source, as the reference implementation copies them. A provider
     * without these operations, or a target that refuses them for this file, leaves a copy with fresh ones. */
    const dotnet_pal_files_ops *f = sn_files(); dotnet_pal_file_status status;
    if (result == 0 && f && f->status(source->handle, &status, sizeof status) == DOTNET_PAL_OK) {
        uint32_t changed = f->set_file_times ? f->set_file_times(destination->handle, status.accessed_ns, status.modified_ns) : DOTNET_PAL_UNSUPPORTED;
        if (changed == DOTNET_PAL_OK || changed == DOTNET_PAL_UNSUPPORTED || changed == DOTNET_PAL_ACCESS_DENIED)
            changed = f->set_file_mode && status.mode != 0 ? f->set_file_mode(destination->handle, status.mode & 0777) : DOTNET_PAL_UNSUPPORTED;
        if (changed != DOTNET_PAL_OK && changed != DOTNET_PAL_UNSUPPORTED && changed != DOTNET_PAL_ACCESS_DENIED) result = sn_status(changed);
    }
    sn_unpin(destination); sn_unpin(source);
    return result;
}

/* ---- paths ------------------------------------------------------------------------- */
typedef uint32_t (*path_call)(const uint8_t*, size_t);
static int32_t by_path(const char *path, path_call call, int absent) {
    if (!call) return sn_fail(absent);
    if (!path || !*path) return sn_fail(ENOENT);
    return sn_status(call((const uint8_t*)path, strlen(path)));
}
PALEXPORT int32_t SystemNative_Unlink(const char* path) { const dotnet_pal_files_ops *f = sn_files(); return by_path(path, f ? f->remove : NULL, ENOENT); }
PALEXPORT int32_t SystemNative_RmDir(const char* path) { const dotnet_pal_files_ops *f = sn_files(); return by_path(path, f ? f->directory_remove : NULL, ENOENT); }
PALEXPORT int32_t SystemNative_MkDir(const char* path, int32_t mode) {
    const dotnet_pal_files_ops *f = sn_files();
    if (!f) return sn_fail(ENOTSUP);
    if (!path || !*path) return sn_fail(ENOENT);
    return sn_status(f->directory_create((const uint8_t*)path, strlen(path), (uint32_t)mode & 07777));
}
PALEXPORT int32_t SystemNative_Rename(const char* oldPath, const char* newPath) {
    const dotnet_pal_files_ops *f = sn_files();
    if (!f || !oldPath || !*oldPath || !newPath || !*newPath) return sn_fail(ENOENT);
    return sn_status(f->rename((const uint8_t*)oldPath, strlen(oldPath), (const uint8_t*)newPath, strlen(newPath)));
}
PALEXPORT char* SystemNative_GetCwd(char* buffer, int32_t bufferSize) {
    const dotnet_pal_files_ops *f = sn_files(); size_t needed = 0;
    if (!f) { errno = ENOTSUP; return NULL; }
    if (!buffer || bufferSize <= 0) { errno = EINVAL; return NULL; }
    uint32_t code = f->current_directory((uint8_t*)buffer, (size_t)bufferSize, &needed);
    if (code != DOTNET_PAL_OK) { errno = sn_errno(code); return NULL; }
    return buffer;
}
/* Optional operations of the files group: a provider without one answers UNSUPPORTED, which is ENOTSUP here. */
typedef uint32_t (*two_path_call)(const uint8_t*, size_t, const uint8_t*, size_t);
static int32_t by_two_paths(const char *first, const char *second, two_path_call call) {
    if (!call) return sn_fail(ENOTSUP);
    if (!first || !*first || !second || !*second) return sn_fail(ENOENT);
    return sn_status(call((const uint8_t*)first, strlen(first), (const uint8_t*)second, strlen(second)));
}
PALEXPORT int32_t SystemNative_Link(const char* source, const char* linkTarget) { const dotnet_pal_files_ops *f = sn_files(); return by_two_paths(source, linkTarget, f ? f->link : NULL); }
PALEXPORT int32_t SystemNative_SymLink(const char* target, const char* linkPath) { const dotnet_pal_files_ops *f = sn_files(); return by_two_paths(target, linkPath, f ? f->symlink : NULL); }
PALEXPORT int32_t SystemNative_ChDir(const char* path) { const dotnet_pal_files_ops *f = sn_files(); return by_path(path, f ? f->set_current_directory : NULL, ENOTSUP); }
PALEXPORT int32_t SystemNative_ChMod(const char* path, int32_t mode) {
    const dotnet_pal_files_ops *f = sn_files();
    if (!f || !f->set_mode) return sn_fail(ENOTSUP);
    if (!path || !*path) return sn_fail(ENOENT);
    return sn_status(f->set_mode((const uint8_t*)path, strlen(path), (uint32_t)mode & 07777));
}
PALEXPORT int32_t SystemNative_FChMod(intptr_t fd, int32_t mode) {
    sn_object *object = sn_pin(fd, SN_FILE, EINVAL);
    if (!object) return -1;
    const dotnet_pal_files_ops *f = sn_files();
    int32_t result = sn_status(f && f->set_file_mode ? f->set_file_mode(object->handle, (uint32_t)mode & 07777) : DOTNET_PAL_UNSUPPORTED);
    sn_unpin(object);
    return result;
}
/* times[0] is the access time and times[1] the modification time; the BCL always passes both. */
static bool time_of(const TimeSpec *time, uint64_t *out) {
    if (time->tv_sec < 0 || time->tv_nsec < 0 || time->tv_nsec >= 1000000000 || time->tv_sec > INT64_MAX / 1000000000 - 1) return false;
    *out = (uint64_t)time->tv_sec * 1000000000u + (uint64_t)time->tv_nsec;
    return true;
}
PALEXPORT int32_t SystemNative_UTimensat(const char* path, TimeSpec* times) {
    const dotnet_pal_files_ops *f = sn_files(); uint64_t accessed, modified;
    if (!f || !f->set_times) return sn_fail(ENOTSUP);
    if (!path || !*path) return sn_fail(ENOENT);
    if (!times || !time_of(&times[0], &accessed) || !time_of(&times[1], &modified)) return sn_fail(EINVAL);
    /* The link itself, as the reference implementation does with AT_SYMLINK_NOFOLLOW. */
    return sn_status(f->set_times((const uint8_t*)path, strlen(path), 0, accessed, modified));
}
PALEXPORT int32_t SystemNative_FUTimens(intptr_t fd, TimeSpec* times) {
    uint64_t accessed, modified;
    if (!times || !time_of(&times[0], &accessed) || !time_of(&times[1], &modified)) return sn_fail(EINVAL);
    sn_object *object = sn_pin(fd, SN_FILE, EINVAL);
    if (!object) return -1;
    const dotnet_pal_files_ops *f = sn_files();
    int32_t result = sn_status(f && f->set_file_times ? f->set_file_times(object->handle, accessed, modified) : DOTNET_PAL_UNSUPPORTED);
    sn_unpin(object);
    return result;
}
/* The hidden flag is a BSD file flag; nothing behind the boundary has one. */
PALEXPORT int32_t SystemNative_LChflags(const char* path, uint32_t flags) { (void)path; (void)flags; return sn_fail(ENOTSUP); }
PALEXPORT int32_t SystemNative_FChflags(intptr_t fd, uint32_t flags) { (void)fd; (void)flags; return sn_fail(ENOTSUP); }
PALEXPORT int32_t SystemNative_LChflagsCanSetHiddenFlag(void) { return 0; }
PALEXPORT int32_t SystemNative_CanGetHiddenFlag(void) { return 0; }
/* The caller frees the result with SystemNative_Free. */
PALEXPORT char* SystemNative_RealPath(const char* path) {
    const dotnet_pal_files_ops *f = sn_files(); size_t needed = 0;
    if (!f || !f->real_path) { errno = ENOTSUP; return NULL; }
    if (!path || !*path) { errno = ENOENT; return NULL; }
    char *text = SystemNative_Malloc(DOTNET_PAL_MAX_NAME + 1);
    if (!text) return NULL;
    uint32_t code = f->real_path((const uint8_t*)path, strlen(path), (uint8_t*)text, DOTNET_PAL_MAX_NAME + 1, &needed);
    if (code != DOTNET_PAL_OK) { SystemNative_Free(text); errno = sn_errno(code); return NULL; }
    return text;
}
/* As readlink(2): the bytes of the target without a terminator, cut to the buffer; the BCL grows the buffer until the text is shorter than it. */
PALEXPORT int32_t SystemNative_ReadLink(const char* path, char* buffer, int32_t bufferSize) {
    const dotnet_pal_files_ops *f = sn_files(); size_t needed = 0;
    if (!path || !*path) return sn_fail(ENOENT);
    if (!buffer || bufferSize <= 0) return sn_fail(EINVAL);
    if (!f || !f->read_link) {
        /* Without the operation the boundary still says whether the path is a link at all: a non-link is EINVAL. */
        FileStatus status;
        if (path_status(path, &status, 0) != 0) return -1;
        return sn_fail((status.Mode & PAL_S_IFMT) == PAL_S_IFLNK ? ENOTSUP : EINVAL);
    }
    char *text = SystemNative_Malloc(DOTNET_PAL_MAX_NAME + 1);
    if (!text) return -1;
    uint32_t code = f->read_link((const uint8_t*)path, strlen(path), (uint8_t*)text, DOTNET_PAL_MAX_NAME + 1, &needed);
    int32_t result = code == DOTNET_PAL_OK ? ((int32_t)(needed - 1) < bufferSize ? (int32_t)(needed - 1) : bufferSize) : sn_fail(sn_errno(code));
    if (result > 0) memcpy(buffer, text, (size_t)result);
    SystemNative_Free(text);
    return result;
}
PALEXPORT int64_t SystemNative_SysConf(int32_t name) {
    const dotnet_pal_api *a = sn_api();
    /* The clock tick is a unit of /proc texts, which are the target's and not the boundary's. */
    if (name == PAL_SC_PAGESIZE && (a->header.capabilities & DOTNET_PAL_CAP_VM) && a->vm.page_size) return (int64_t)a->vm.page_size();
    return sn_fail(EINVAL);
}

/* ---- temporary files ------------------------------------------------------------------ */
int32_t SystemNative_GetCryptographicallySecureRandomBytes(uint8_t* buffer, int32_t bufferLength);
void SystemNative_GetNonCryptographicallySecureRandomBytes(uint8_t* buffer, int32_t bufferLength);
/* Replaces the XXXXXX before the suffix and creates the file exclusively, retrying on a name that exists. */
PALEXPORT intptr_t SystemNative_MksTemps(char* pathTemplate, int32_t suffixLength) {
    static const char alphabet[] = "abcdefghijklmnopqrstuvwxyz0123456789";
    size_t length = pathTemplate ? strlen(pathTemplate) : 0;
    if (suffixLength < 0 || length < (size_t)suffixLength + 6) return sn_fail(EINVAL);
    char *marks = pathTemplate + length - (size_t)suffixLength - 6;
    if (memcmp(marks, "XXXXXX", 6) != 0) return sn_fail(EINVAL);
    for (int attempt = 0; attempt < 64; ++attempt) {
        uint8_t random[6];
        SystemNative_GetNonCryptographicallySecureRandomBytes(random, 6);
        for (int i = 0; i < 6; ++i) marks[i] = alphabet[random[i] % 36];
        intptr_t fd = SystemNative_Open(pathTemplate, PAL_O_RDWR | PAL_O_CREAT | PAL_O_EXCL | PAL_O_CLOEXEC, 0600);
        if (fd >= 0 || errno != EEXIST) return fd;
    }
    return sn_fail(EEXIST);
}
/* The same for a directory, as mkdtemp(3): the template comes back filled in, or NULL with errno. */
PALEXPORT char* SystemNative_MkdTemp(char* pathTemplate) {
    static const char alphabet[] = "abcdefghijklmnopqrstuvwxyz0123456789";
    const dotnet_pal_files_ops *f = sn_files();
    size_t length = pathTemplate ? strlen(pathTemplate) : 0;
    if (!f) { errno = ENOTSUP; return NULL; }
    if (length < 6 || memcmp(pathTemplate + length - 6, "XXXXXX", 6) != 0) { errno = EINVAL; return NULL; }
    for (int attempt = 0; attempt < 64; ++attempt) {
        uint8_t random[6];
        SystemNative_GetNonCryptographicallySecureRandomBytes(random, 6);
        for (int i = 0; i < 6; ++i) pathTemplate[length - 6 + (size_t)i] = alphabet[random[i] % 36];
        uint32_t status = f->directory_create((const uint8_t*)pathTemplate, length, 0700);
        if (status == DOTNET_PAL_OK) return pathTemplate;
        if (status != DOTNET_PAL_ALREADY_EXISTS) { errno = sn_errno(status); return NULL; }
    }
    errno = EEXIST;
    return NULL;
}

/* ---- change watching ------------------------------------------------------ */
/* FileSystemWatcher speaks inotify: a descriptor that Read fills with event records and Poll can ask about. A record is
 * { int32 wd; uint32 mask; uint32 cookie; uint32 len; char name[len]; }. One thread reads, as the contract of the group
 * wants it; the event a Poll has seen and a Read has not taken yet waits in the state below. */
struct sn_watcher { void *watcher; bool pending; dotnet_pal_watch_event event; };
/* A wait is cut into slices so a reader learns that its descriptor was closed under it. */
#define WATCH_SLICE_NS UINT64_C(1000000000)
static const struct { uint32_t native, boundary; } watch_bits[] = {
    {PAL_IN_ACCESS, DOTNET_PAL_WATCH_ACCESS}, {PAL_IN_MODIFY, DOTNET_PAL_WATCH_MODIFY}, {PAL_IN_ATTRIB, DOTNET_PAL_WATCH_ATTRIBUTES},
    {PAL_IN_MOVED_FROM, DOTNET_PAL_WATCH_MOVED_FROM}, {PAL_IN_MOVED_TO, DOTNET_PAL_WATCH_MOVED_TO}, {PAL_IN_CREATE, DOTNET_PAL_WATCH_CREATE},
    {PAL_IN_DELETE, DOTNET_PAL_WATCH_DELETE}, {PAL_IN_Q_OVERFLOW, DOTNET_PAL_WATCH_OVERFLOW}, {PAL_IN_IGNORED, DOTNET_PAL_WATCH_REMOVED},
    {PAL_IN_ISDIR, DOTNET_PAL_WATCH_DIRECTORY}, {PAL_IN_ONLYDIR, DOTNET_PAL_WATCH_ONLY_DIRECTORY}, {PAL_IN_DONT_FOLLOW, DOTNET_PAL_WATCH_NO_FOLLOW},
};
static uint32_t watch_translate(uint32_t bits, bool to_boundary) {
    uint32_t out = 0;
    for (size_t i = 0; i < sizeof watch_bits / sizeof *watch_bits; ++i)
        if (bits & (to_boundary ? watch_bits[i].native : watch_bits[i].boundary)) out |= to_boundary ? watch_bits[i].boundary : watch_bits[i].native;
    return out;
}
static bool watch_closed(sn_object *object) { sn_lock(); bool closed = object->descriptors == 0; sn_unlock(); return closed; }
/* 1: an event waits in the state; 0: none came in time, or the descriptor was closed; -1: errno. */
static int watch_fill(sn_object *object, int64_t timeout_ns) {
    const dotnet_pal_watches_ops *w = sn_watches(); struct sn_watcher *state = object->handle;
    if (!w) return sn_fail(EBADF);
    while (!state->pending) {
        uint64_t slice = timeout_ns < 0 || (uint64_t)timeout_ns > WATCH_SLICE_NS ? WATCH_SLICE_NS : (uint64_t)timeout_ns;
        uint32_t status = w->read(state->watcher, slice, &state->event, sizeof state->event);
        if (status == DOTNET_PAL_OK) { state->pending = true; break; }
        if (status != DOTNET_PAL_TIMEOUT) return sn_status(status);
        if (timeout_ns >= 0 && (timeout_ns -= (int64_t)slice) <= 0) return 0;
        if (watch_closed(object)) return 0;
    }
    return 1;
}
bool sn_watch_ready(sn_object *object, int32_t timeout_ms) { return watch_fill(object, timeout_ms < 0 ? -1 : (int64_t)timeout_ms * 1000000) == 1; }
static int32_t watch_read(sn_object *object, uint8_t *buffer, int32_t size) {
    struct sn_watcher *state = object->handle; int32_t used = 0;
    for (;;) {
        /* The first event is waited for; whatever else is there already rides along, as one read of inotify returns it. */
        int filled = watch_fill(object, used ? 0 : -1);
        if (filled <= 0) return used ? used : filled;
        const dotnet_pal_watch_event *event = &state->event;
        uint32_t padded = event->name_length ? (event->name_length + 4u) & ~3u : 0, record = 16 + padded;
        if (record > (uint32_t)(size - used)) return used ? used : sn_fail(EINVAL);
        int32_t wd = event->watch == 0 || event->watch > INT32_MAX ? -1 : (int32_t)event->watch;
        uint32_t mask = watch_translate(event->events, false);
        memcpy(buffer + used, &wd, 4); memcpy(buffer + used + 4, &mask, 4); memcpy(buffer + used + 8, &event->cookie, 4); memcpy(buffer + used + 12, &padded, 4);
        memset(buffer + used + 16, 0, padded); memcpy(buffer + used + 16, event->name, event->name_length);
        used += (int32_t)record; state->pending = false;
    }
}
static void watch_destroy(sn_object *object) {
    const dotnet_pal_watches_ops *w = sn_watches(); struct sn_watcher *state = object->handle;
    if (state && w) (void)w->close(state->watcher);
    SystemNative_Free(state);
}
PALEXPORT intptr_t SystemNative_INotifyInit(void) {
    const dotnet_pal_watches_ops *w = sn_watches();
    if (!w) return sn_fail(ENOTSUP);
    struct sn_watcher *state = SystemNative_Calloc(1, sizeof *state);
    if (!state) return sn_fail(ENOMEM);
    uint32_t status = w->open(&state->watcher);
    /* The limit of watchers is EMFILE to inotify_init, which is how FileSystemWatcher tells it from other failures. */
    if (status != DOTNET_PAL_OK) { SystemNative_Free(state); return sn_fail(status == DOTNET_PAL_NO_SPACE ? EMFILE : sn_errno(status)); }
    sn_object *object = sn_new(SN_WATCH, state);
    if (!object) { (void)w->close(state->watcher); SystemNative_Free(state); return sn_fail(ENOMEM); }
    return sn_install(object);
}
PALEXPORT int32_t SystemNative_INotifyAddWatch(intptr_t fd, const char* pathName, uint32_t mask) {
    const dotnet_pal_watches_ops *w = sn_watches(); uint32_t watch = 0;
    if (!pathName) return sn_fail(EFAULT);
    sn_object *object = sn_pin(fd, SN_WATCH, EINVAL);
    if (!object) return -1;
    /* IN_EXCL_UNLINK has no counterpart: what an unlinked entry still does is not something every target can tell. */
    uint32_t status = w ? w->add(((struct sn_watcher*)object->handle)->watcher, (const uint8_t*)pathName, strlen(pathName), watch_translate(mask, true), &watch) : DOTNET_PAL_UNSUPPORTED;
    if (status == DOTNET_PAL_OK && watch > INT32_MAX) { (void)w->remove(((struct sn_watcher*)object->handle)->watcher, watch); status = DOTNET_PAL_NO_SPACE; }
    sn_unpin(object);
    return status == DOTNET_PAL_OK ? (int32_t)watch : sn_status(status);
}
PALEXPORT int32_t SystemNative_INotifyRemoveWatch(intptr_t fd, int32_t wd) {
    const dotnet_pal_watches_ops *w = sn_watches();
    if (wd <= 0) return sn_fail(EINVAL);
    sn_object *object = sn_pin(fd, SN_WATCH, EINVAL);
    if (!object) return -1;
    uint32_t status = w ? w->remove(((struct sn_watcher*)object->handle)->watcher, (uint32_t)wd) : DOTNET_PAL_UNSUPPORTED;
    sn_unpin(object);
    return sn_status(status);
}

/* ---- file mappings -------------------------------------------------------- */
/* MMap without a descriptor is the runtime group's anonymous memory; with one it is the mappings group. MUnmap and MSync
 * get an address only, so the unit remembers which ranges came from a file. */
#define SN_MAX_MAPPINGS 256
static struct { void *address; size_t length; } file_mappings[SN_MAX_MAPPINGS];
static const dotnet_pal_runtime_ops *anonymous(void) {
    const dotnet_pal_api *a = sn_api();
    return sn_has(a, DOTNET_PAL_RUNTIME_API_SIZE, DOTNET_PAL_CAP_NATIVE_MEMORY) && a->runtime.mapping_allocate ? &a->runtime : NULL;
}
static uint32_t protection_bits(int32_t p) {
    return ((p & PAL_PROT_READ) ? DOTNET_PAL_READ : 0) | ((p & PAL_PROT_WRITE) ? DOTNET_PAL_WRITE : 0) | ((p & PAL_PROT_EXEC) ? DOTNET_PAL_EXECUTE : 0);
}
static int mapping_slot(void *address) { for (int i = 0; i < SN_MAX_MAPPINGS; ++i) if (file_mappings[i].address == address) return i; return -1; }
PALEXPORT void* SystemNative_MMap(void* address, uint64_t length, int32_t protection, int32_t flags, intptr_t fd, int64_t offset) {
    void *p = NULL;
    (void)address; /* a hint the BCL never gives */
    if (length == 0 || length > SIZE_MAX) { errno = EINVAL; return NULL; }
    if (flags & PAL_MAP_ANONYMOUS) {
        const dotnet_pal_runtime_ops *r = anonymous();
        if (!r) { errno = ENOTSUP; return NULL; }
        if (r->mapping_allocate((size_t)length, protection_bits(protection), &p) != DOTNET_PAL_OK) { errno = ENOMEM; return NULL; }
        return p;
    }
    const dotnet_pal_mappings_ops *m = sn_mappings();
    uint32_t mode = (flags & PAL_MAP_SHARED) ? DOTNET_PAL_MAP_SHARED : (flags & PAL_MAP_PRIVATE) ? DOTNET_PAL_MAP_PRIVATE : 0;
    if (!m) { errno = ENOTSUP; return NULL; }
    if (offset < 0 || mode == 0 || protection_bits(protection) == 0) { errno = EINVAL; return NULL; }
    sn_object *object = sn_pin(fd, SN_FILE, ENODEV);
    if (!object) return NULL;
    /* The slot is taken first: a mapping nobody could unmap again is worse than a refused one. */
    sn_lock();
    int slot = mapping_slot(NULL);
    if (slot >= 0) file_mappings[slot].address = (void*)file_mappings; /* reserved: no mapping has the table's own address */
    sn_unlock();
    if (slot < 0) { sn_unpin(object); errno = ENOMEM; return NULL; }
    uint32_t status = m->map(object->handle, (uint64_t)offset, (size_t)length, protection_bits(protection), mode, &p);
    sn_unpin(object);
    sn_lock();
    file_mappings[slot].address = status == DOTNET_PAL_OK ? p : NULL; file_mappings[slot].length = (size_t)length;
    sn_unlock();
    if (status != DOTNET_PAL_OK) { errno = sn_errno(status); return NULL; }
    return p;
}
PALEXPORT int32_t SystemNative_MUnmap(void* address, uint64_t length) {
    if (!address || length == 0 || length > SIZE_MAX) return sn_fail(EINVAL);
    sn_lock();
    int slot = mapping_slot(address);
    size_t mapped = slot >= 0 ? file_mappings[slot].length : 0;
    sn_unlock();
    if (slot >= 0) {
        const dotnet_pal_mappings_ops *m = sn_mappings();
        /* The boundary unmaps what it mapped, whole: a partial unmap is not something the BCL does. */
        if (!m || mapped != (size_t)length) return sn_fail(EINVAL);
        uint32_t status = m->unmap(address, mapped);
        if (status == DOTNET_PAL_OK) { sn_lock(); file_mappings[slot].address = NULL; sn_unlock(); }
        return sn_status(status);
    }
    const dotnet_pal_runtime_ops *r = anonymous();
    if (!r) return sn_fail(ENOTSUP);
    return r->mapping_release(address, (size_t)length) == DOTNET_PAL_OK ? 0 : sn_fail(EINVAL);
}
PALEXPORT int32_t SystemNative_MProtect(void* address, uint64_t length, int32_t protection) {
    const dotnet_pal_runtime_ops *r = anonymous();
    if (!r) return sn_fail(ENOTSUP);
    return r->mapping_protect(address, (size_t)length, protection_bits(protection)) == DOTNET_PAL_OK ? 0 : sn_fail(EINVAL);
}
PALEXPORT int32_t SystemNative_MSync(void* address, uint64_t length, int32_t flags) {
    (void)flags; (void)length;
    sn_lock();
    int slot = mapping_slot(address);
    size_t mapped = slot >= 0 ? file_mappings[slot].length : 0;
    sn_unlock();
    /* Anonymous memory has nothing to write back. */
    if (slot < 0) return 0;
    const dotnet_pal_mappings_ops *m = sn_mappings();
    return m ? sn_status(m->sync(address, mapped)) : sn_fail(EINVAL);
}
/* Advice changes no result, and the one the BCL gives (keep the pages out of a forked child) has no subject here:
 * the boundary starts children without copying the address space. */
PALEXPORT int32_t SystemNative_MAdvise(void* address, uint64_t length, int32_t advice) { (void)address; (void)length; (void)advice; return 0; }
/* No shared memory objects: MemoryMappedFile falls back to an unlinked temporary file, which the files group carries. */
PALEXPORT int32_t SystemNative_IsMemfdSupported(void) { return 0; }
PALEXPORT intptr_t SystemNative_MemfdCreate(const char* name, int32_t isReadonly) { (void)name; (void)isReadonly; return sn_fail(ENOTSUP); }
PALEXPORT intptr_t SystemNative_ShmOpen(const char* name, int32_t flags, int32_t mode) { (void)name; (void)flags; (void)mode; return sn_fail(ENOTSUP); }
PALEXPORT int32_t SystemNative_ShmUnlink(const char* name) { (void)name; return sn_fail(ENOTSUP); }

/* ---- volumes -------------------------------------------------------------- */
PALEXPORT int32_t SystemNative_GetAllMountPoints(MountPointFound onFound, void* context) {
    const dotnet_pal_volumes_ops *v = sn_volumes();
    if (!onFound) return sn_fail(EFAULT);
    if (!v) return sn_fail(ENOTSUP);
    char *name = SystemNative_Malloc(4096);
    if (!name) return sn_fail(ENOMEM);
    int32_t result = 0;
    for (size_t index = 0; index < 65536; ++index) {
        size_t needed = 0;
        uint32_t status = v->entry(index, (uint8_t*)name, 4096, &needed);
        if (status == DOTNET_PAL_NOT_FOUND) break;
        if (status != DOTNET_PAL_OK) { result = sn_status(status); break; }
        onFound(context, name);
    }
    SystemNative_Free(name);
    return result;
}
static int32_t volume_status(const char *name, dotnet_pal_volume_status *out) {
    const dotnet_pal_volumes_ops *v = sn_volumes();
    if (!name) return sn_fail(EFAULT);
    if (!v) return sn_fail(ENOTSUP);
    return sn_status(v->status((const uint8_t*)name, strlen(name), out, sizeof *out));
}
PALEXPORT int32_t SystemNative_GetSpaceInfoForMountPoint(const char* name, MountPointInformation* mpi) {
    dotnet_pal_volume_status status;
    if (!mpi) return sn_fail(EFAULT);
    memset(mpi, 0, sizeof *mpi);
    if (volume_status(name, &status) != 0) return -1;
    mpi->AvailableFreeSpace = status.available_bytes; mpi->TotalFreeSpace = status.free_bytes; mpi->TotalSize = status.total_bytes;
    return 0;
}
PALEXPORT int32_t SystemNative_GetFileSystemTypeNameForMountPoint(const char* name, char* formatNameBuffer, int32_t bufferLength, int64_t* formatType) {
    dotnet_pal_volume_status status;
    if (!formatNameBuffer || !formatType || bufferLength <= 0) return sn_fail(EFAULT);
    formatNameBuffer[0] = 0; *formatType = -1; /* -1: the name is in the buffer, not a number of the reference list */
    if (volume_status(name, &status) != 0) return -1;
    size_t length = strlen((const char*)status.format);
    if (length >= (size_t)bufferLength) return sn_fail(ERANGE);
    memcpy(formatNameBuffer, status.format, length + 1);
    return 0;
}

/* ---- directories ------------------------------------------------------------------------- */
/* The BCL treats DIR as opaque. The name of the current entry stays valid until the next ReadDir or CloseDir. */
struct sn_directory { void *handle; char name[DOTNET_PAL_MAX_ENTRY_NAME + 1]; };
PALEXPORT struct sn_directory* SystemNative_OpenDir(const char* path) {
    const dotnet_pal_files_ops *f = sn_files(); void *handle = NULL;
    if (!f || !path || !*path) { errno = ENOENT; return NULL; }
    uint32_t code = f->directory_open((const uint8_t*)path, strlen(path), &handle);
    if (code != DOTNET_PAL_OK) { errno = sn_errno(code); return NULL; }
    struct sn_directory *directory = SystemNative_Calloc(1, sizeof *directory);
    if (!directory) { (void)f->directory_close(handle); errno = ENOMEM; return NULL; }
    directory->handle = handle;
    return directory;
}
/* 0 with an entry, -1 at the end, an errno value on failure (this entry point does not use errno). */
PALEXPORT int32_t SystemNative_ReadDir(struct sn_directory* directory, DirectoryEntry* outputEntry) {
    static const int32_t types[] = {PAL_DT_UNKNOWN, PAL_DT_REG, PAL_DT_DIR, PAL_DT_LNK, PAL_DT_UNKNOWN};
    const dotnet_pal_files_ops *f = sn_files(); size_t length = 0; uint32_t kind = 0;
    memset(outputEntry, 0, sizeof *outputEntry);
    if (!f || !directory) return EBADF;
    uint32_t code = f->directory_read(directory->handle, (uint8_t*)directory->name, DOTNET_PAL_MAX_ENTRY_NAME, &length, &kind);
    if (code == DOTNET_PAL_NOT_FOUND) return -1;
    if (code != DOTNET_PAL_OK) return sn_errno(code);
    directory->name[length] = 0;
    outputEntry->Name = directory->name; outputEntry->NameLength = (int32_t)length;
    outputEntry->InodeType = types[kind <= DOTNET_PAL_NODE_OTHER ? kind : 0];
    return 0;
}
PALEXPORT int32_t SystemNative_CloseDir(struct sn_directory* directory) {
    const dotnet_pal_files_ops *f = sn_files();
    if (!directory) return sn_fail(EBADF);
    uint32_t code = f ? f->directory_close(directory->handle) : DOTNET_PAL_OK;
    SystemNative_Free(directory);
    return sn_status(code);
}

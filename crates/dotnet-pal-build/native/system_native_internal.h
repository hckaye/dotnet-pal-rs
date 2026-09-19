#ifndef DOTNET_PAL_SYSTEM_NATIVE_INTERNAL_H
#define DOTNET_PAL_SYSTEM_NATIVE_INTERNAL_H
/* Shared by the units of System.Native over the boundary: system_native_pal.c
 * (heap, threads, clocks, environment, system facts, notifications, terminal),
 * system_native_io.c (the descriptor table, standard streams, files and
 * directories, change watching, file mappings, volumes), system_native_net.c
 * (sockets, readiness events, name resolution, network interfaces) and system_native_proc.c (child processes and their pipes). Compiled
 * against glibc headers for the errno values the managed mapping expects; none
 * of the units makes a libc call beyond the freestanding CRT contract. */
#define _GNU_SOURCE
#include "dotnet_pal.h"
#include "system_native_abi.h"
#include <errno.h>
#include <limits.h>
#include <stdarg.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define PALEXPORT __attribute__((__visibility__("default")))

/* ---- table access ------------------------------------------------------- */
static inline const dotnet_pal_api *sn_api(void) {
    const dotnet_pal_api *a = dotnet_pal_get_api(DOTNET_PAL_ABI_VERSION);
    if (!a || a->header.abi_version != DOTNET_PAL_ABI_VERSION) __builtin_trap();
    return a;
}
static inline bool sn_has(const dotnet_pal_api *a, size_t size, uint64_t capability) {
    return a->header.struct_size >= size && (a->header.capabilities & capability) == capability;
}
static inline int sn_fail(int error) { errno = error; return -1; }
static inline const dotnet_pal_files_ops *sn_files(void) {
    const dotnet_pal_api *a = sn_api();
    return sn_has(a, DOTNET_PAL_FILES_API_SIZE, DOTNET_PAL_CAP_FILES) ? &a->files : NULL;
}
static inline const dotnet_pal_sockets_ops *sn_sockets(void) {
    const dotnet_pal_api *a = sn_api();
    return sn_has(a, DOTNET_PAL_SOCKETS_API_SIZE, DOTNET_PAL_CAP_SOCKETS) ? &a->sockets : NULL;
}
static inline const dotnet_pal_processes_ops *sn_processes(void) {
    const dotnet_pal_api *a = sn_api();
    return sn_has(a, DOTNET_PAL_PROCESSES_API_SIZE, DOTNET_PAL_CAP_PROCESSES) ? &a->processes : NULL;
}
static inline const dotnet_pal_watches_ops *sn_watches(void) {
    const dotnet_pal_api *a = sn_api();
    return sn_has(a, DOTNET_PAL_WATCHES_API_SIZE, DOTNET_PAL_CAP_WATCHES) ? &a->watches : NULL;
}
static inline const dotnet_pal_mappings_ops *sn_mappings(void) {
    const dotnet_pal_api *a = sn_api();
    return sn_has(a, DOTNET_PAL_MAPPINGS_API_SIZE, DOTNET_PAL_CAP_MAPPINGS) ? &a->mappings : NULL;
}
static inline const dotnet_pal_volumes_ops *sn_volumes(void) {
    const dotnet_pal_api *a = sn_api();
    return sn_has(a, DOTNET_PAL_VOLUMES_API_SIZE, DOTNET_PAL_CAP_VOLUMES) ? &a->volumes : NULL;
}
static inline const dotnet_pal_network_ops *sn_network(void) {
    const dotnet_pal_api *a = sn_api();
    return sn_has(a, DOTNET_PAL_NETWORK_API_SIZE, DOTNET_PAL_CAP_NETWORK) ? &a->network : NULL;
}
static inline const dotnet_pal_local_sockets_ops *sn_local_sockets(void) {
    const dotnet_pal_api *a = sn_api();
    return sn_has(a, DOTNET_PAL_LOCAL_SOCKETS_API_SIZE, DOTNET_PAL_CAP_LOCAL_SOCKETS) ? &a->local_sockets : NULL;
}
static inline const dotnet_pal_accounts_ops *sn_accounts(void) {
    const dotnet_pal_api *a = sn_api();
    return sn_has(a, DOTNET_PAL_ACCOUNTS_API_SIZE, DOTNET_PAL_CAP_ACCOUNTS) ? &a->accounts : NULL;
}
static inline const dotnet_pal_priority_ops *sn_priority(void) {
    const dotnet_pal_api *a = sn_api();
    return sn_has(a, DOTNET_PAL_PRIORITY_API_SIZE, DOTNET_PAL_CAP_PRIORITY) ? &a->priority : NULL;
}
static inline const dotnet_pal_packets_ops *sn_packets(void) {
    const dotnet_pal_api *a = sn_api();
    return sn_has(a, DOTNET_PAL_PACKETS_API_SIZE, DOTNET_PAL_CAP_PACKETS) ? &a->packets : NULL;
}
static inline const dotnet_pal_spawn_as_ops *sn_spawn_as(void) {
    const dotnet_pal_api *a = sn_api();
    return sn_has(a, DOTNET_PAL_SPAWN_AS_API_SIZE, DOTNET_PAL_CAP_SPAWN_AS) ? &a->spawn_as : NULL;
}
static inline const dotnet_pal_streams_ops *sn_streams(void) {
    const dotnet_pal_api *a = sn_api();
    if (!sn_has(a, DOTNET_PAL_STREAMS_API_SIZE, DOTNET_PAL_CAP_STREAMS) || !a->streams.write || !a->streams.read || !a->streams.is_terminal) return NULL;
    return &a->streams;
}
/* The errno the managed side expects for a boundary status. */
int sn_errno(uint32_t status);
static inline int sn_status(uint32_t status) { return status == DOTNET_PAL_OK ? 0 : sn_fail(sn_errno(status)); }

void *SystemNative_Malloc(uintptr_t size);
void *SystemNative_Calloc(uintptr_t num, uintptr_t size);
void SystemNative_Free(void *ptr);
int32_t SystemNative_ConvertErrorPlatformToPal(int32_t error);
int32_t SystemNative_Close(intptr_t fd);
int32_t SystemNative_PRead(intptr_t fd, void *buffer, int32_t bufferSize, int64_t fileOffset);

/* ---- descriptors --------------------------------------------------------- */
/* Managed code names everything it opens by a small integer. Each descriptor
 * refers to one object; Dup makes a second descriptor for the same object, so
 * both share the file position, as POSIX descriptors do. An object lives until
 * its last descriptor is closed and its last user has let go: an operation pins
 * the object for its duration, so a Close from another thread never frees a
 * handle the boundary is still working on. */
#define SN_MAX_FD 1024
typedef enum { SN_STREAM = 1, SN_FILE, SN_SOCKET, SN_PORT, SN_PIPE, SN_WATCH } sn_kind;
struct sn_port;
struct sn_pipe_reader;
typedef struct sn_object {
    sn_kind kind;
    int32_t references;      /* descriptors plus pins; under the table lock */
    int32_t descriptors;     /* descriptors alone: the object is closed for its users when this reaches zero */
    void *handle;            /* boundary file, socket or pipe; SN_WATCH: the unit's own watcher state */
    int32_t stream;          /* SN_STREAM: 0, 1 or 2 */
    /* SN_FILE */
    uint64_t position;       /* cursor of Read, Write and LSeek; positional calls ignore it */
    int32_t open_flags;      /* PAL_O_* */
    /* SN_SOCKET */
    int32_t family, type, protocol;
    bool listening, nonblocking;
    struct sn_port *port;    /* readiness registration, under the table lock */
    uintptr_t port_data;
    int32_t port_events;     /* SocketEvents the engine asked for */
    int32_t port_reported;   /* SocketEvents delivered and not re-armed since */
    struct sn_object *port_next;
    /* SN_PORT */
    struct sn_port *self;
    /* SN_PIPE */
    struct sn_pipe_reader *reader; /* set once a read end is read without blocking; see system_native_proc.c */
} sn_object;

void sn_lock(void);
void sn_unlock(void);
/* Pins the object of fd when it has the kind (0 = any); NULL with errno EBADF or wrong_kind. */
sn_object *sn_pin(intptr_t fd, sn_kind kind, int wrong_kind);
void sn_unpin(sn_object *object);
/* Takes ownership of a fresh object with one reference; -1 with errno EMFILE (the object is destroyed). */
intptr_t sn_install(sn_object *object);
sn_object *sn_new(sn_kind kind, void *handle);
/* Implemented by the network unit. sn_socket_closing is called under the table lock when the last descriptor of a
 * registered socket or pipe closes: it drops the registration and returns the port to wake with sn_port_wake
 * once the lock is released. */
struct sn_port *sn_socket_closing(sn_object *object);
void sn_port_wake(struct sn_port *port);
/* Re-arms edge notification after an operation reported it would block. */
void sn_socket_would_block(sn_object *object, int32_t events);
/* Tells the port an object is registered with that its readiness changed (call without the table lock). */
void sn_port_notify(sn_object *object);
void sn_port_destroy(struct sn_port *port);
/* Implemented by the process unit: the read end of a child's pipe. sn_pipe_read is Read for a pipe in either
 * mode; sn_pipe_watch makes sure its readiness can be observed; sn_pipe_readable answers under the table lock;
 * sn_pipe_closing runs under the table lock when the last descriptor closes; sn_pipe_destroy with the object. */
/* Implemented by the I/O unit: whether a read of a change watcher would return an event within timeout_ms (-1: no limit). */
bool sn_watch_ready(sn_object *object, int32_t timeout_ms);
int32_t sn_pipe_read(sn_object *object, void *buffer, int32_t size);
bool sn_pipe_watch(sn_object *object);
bool sn_pipe_readable(const sn_object *object);
void sn_pipe_closing(sn_object *object);
void sn_pipe_destroy(sn_object *object);
#endif

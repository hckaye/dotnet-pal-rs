/* Independent POSIX reference provider for the host-local-sockets conformance
 * suite, on the descriptors of the sockets reference provider. It keeps nothing
 * about a socket: the domain, the type, whether it listens and whether it has a
 * name are asked of the descriptor each time. Fault 1 withholds a callback;
 * fault 2 answers in the ways the front end is documented to sanitize. */
#define _GNU_SOURCE
#include "dotnet_pal.h"
#include <errno.h>
#include <fcntl.h>
#include <stddef.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>
int pal_local_sockets_fault;
int pal_sockets_host_descriptor(void *socket);
static int own(int fd, int name) { int value = -1; socklen_t length = sizeof value; return getsockopt(fd, SOL_SOCKET, name, &value, &length) == 0 ? value : -1; }
static uint32_t status_of(int code) {
    switch (code) {
    case ENOENT: return DOTNET_PAL_NOT_FOUND;
    case EACCES: case EPERM: return DOTNET_PAL_ACCESS_DENIED;
    case ENOTDIR: return DOTNET_PAL_NOT_DIRECTORY;
    case EROFS: return DOTNET_PAL_READ_ONLY;
    case ENAMETOOLONG: return DOTNET_PAL_NAME_TOO_LONG;
    case EADDRINUSE: return DOTNET_PAL_ADDRESS_IN_USE;
    case ECONNREFUSED: case EPROTOTYPE: return DOTNET_PAL_CONNECTION_REFUSED; /* nobody accepts this kind of connection at the path */
    case EINPROGRESS: return DOTNET_PAL_IN_PROGRESS;
    case EISCONN: return DOTNET_PAL_ALREADY_CONNECTED;
    case ENOTCONN: return DOTNET_PAL_NOT_CONNECTED;
    case ETIMEDOUT: return DOTNET_PAL_TIMEOUT;
    case EINVAL: return DOTNET_PAL_INVALID_ARGUMENT;
    case ENOMEM: case ENOBUFS: return DOTNET_PAL_OUT_OF_MEMORY;
    default: return DOTNET_PAL_OS_ERROR;
    }
}
/* The length bind and connect take, or 0 for a path that leaves no room for its terminator. */
static socklen_t to_native(const uint8_t *path, size_t path_length, struct sockaddr_un *out) {
    memset(out, 0, sizeof *out);
    if (path_length >= sizeof out->sun_path) return 0;
    out->sun_family = AF_UNIX; memcpy(out->sun_path, path, path_length);
    return (socklen_t)(offsetof(struct sockaddr_un, sun_path) + path_length + 1);
}
static uint32_t local_bind(void *socket, const uint8_t *path, size_t path_length) {
    static int step;
    if (pal_local_sockets_fault == 2) switch (step++) {
    case 0: return DOTNET_PAL_IS_DIRECTORY;        /* a status of the files group */
    case 1: return 99u;                            /* no such status */
    default: return DOTNET_PAL_CONNECTION_REFUSED; /* a status only connect has */
    }
    int fd = pal_sockets_host_descriptor(socket); struct sockaddr_un native, held; socklen_t length = to_native(path, path_length, &native), held_length = sizeof held;
    if (own(fd, SO_DOMAIN) != AF_UNIX) return DOTNET_PAL_INVALID_ARGUMENT;
    if (length == 0) return DOTNET_PAL_NAME_TOO_LONG;
    /* The kernel reports the path of a bound socket as taken when it is the socket's own: a socket with a name is refused first. */
    if (getsockname(fd, (struct sockaddr*)&held, &held_length) != 0) return status_of(errno);
    if (held_length > offsetof(struct sockaddr_un, sun_path)) return DOTNET_PAL_INVALID_ARGUMENT;
    return bind(fd, (struct sockaddr*)&native, length) == 0 ? DOTNET_PAL_OK : status_of(errno);
}
static uint32_t local_connect(void *socket, const uint8_t *path, size_t path_length) {
    static int step;
    if (pal_local_sockets_fault == 2) switch (step++) {
    case 0: return DOTNET_PAL_ADDRESS_IN_USE;      /* a status only bind has */
    case 1: return DOTNET_PAL_BUFFER_TOO_SMALL;    /* a status only address has */
    default: return 99u;
    }
    int fd = pal_sockets_host_descriptor(socket), rc; struct sockaddr_un native; socklen_t length = to_native(path, path_length, &native);
    if (own(fd, SO_DOMAIN) != AF_UNIX) return DOTNET_PAL_INVALID_ARGUMENT;
    if (length == 0) return DOTNET_PAL_NAME_TOO_LONG;
    do rc = connect(fd, (struct sockaddr*)&native, length); while (rc != 0 && errno == EINTR);
    if (rc == 0) return DOTNET_PAL_OK;
    /* A full backlog is EAGAIN: "not now" for a non-blocking socket, an expired SEND_TIMEOUT for a blocking one. */
    if (errno == EAGAIN) return fcntl(fd, F_GETFL) & O_NONBLOCK ? DOTNET_PAL_WOULD_BLOCK : DOTNET_PAL_TIMEOUT;
    return status_of(errno);
}
/* A broken answer: the bytes as they are, the length the provider states and its status. */
static uint32_t broken(uint8_t *out, size_t capacity, size_t *needed, const char *bytes, size_t size, size_t stated, uint32_t status) {
    if (size <= capacity) memcpy(out, bytes, size);
    *needed = stated; return status;
}
static uint32_t local_address(void *socket, uint32_t peer, uint8_t *out, size_t capacity, size_t *needed) {
    static int step;
    if (pal_local_sockets_fault == 2) switch (step++) {
    case 0: return broken(out, capacity, needed, "/run/sX", 7, 7, DOTNET_PAL_OK);                 /* no terminator */
    case 1: return broken(out, capacity, needed, "/ru\0/s", 7, 7, DOTNET_PAL_OK);                 /* terminator inside the text */
    case 2: return broken(out, capacity, needed, "", 1, 1, DOTNET_PAL_OK);                        /* a text of length 1 is no path */
    case 3: return broken(out, capacity, needed, "/run/s", 7, 0, DOTNET_PAL_OK);                  /* no length */
    case 4: return broken(out, capacity, needed, "/run/s", 7, 7, DOTNET_PAL_IS_DIRECTORY);        /* a status of the files group */
    case 5: return broken(out, capacity, needed, "/run/s", 7, 7, DOTNET_PAL_ADDRESS_IN_USE);      /* a status only bind has */
    case 6: return broken(out, capacity, needed, "/run/s", 7, DOTNET_PAL_MAX_NAME + 2, DOTNET_PAL_BUFFER_TOO_SMALL); /* longer than any path */
    case 7: return broken(out, capacity, needed, "", 0, 7, DOTNET_PAL_BUFFER_TOO_SMALL);          /* too small for a text that fits */
    case 8: return broken(out, capacity, needed, "/run/s", 7, 7, DOTNET_PAL_NOT_CONNECTED);       /* outputs written by a failing call */
    default: return broken(out, capacity, needed, "/run/s", 7, 7, DOTNET_PAL_NOT_FOUND);
    }
    int fd = pal_sockets_host_descriptor(socket); struct sockaddr_storage native; socklen_t length = sizeof native;
    if (own(fd, SO_DOMAIN) != AF_UNIX) return DOTNET_PAL_INVALID_ARGUMENT;
    memset(&native, 0, sizeof native); /* a name that fills sun_path arrives without a terminator, and the storage is longer than it */
    if ((peer ? getpeername : getsockname)(fd, (struct sockaddr*)&native, &length) != 0) return status_of(errno);
    const char *path = ((struct sockaddr_un*)&native)->sun_path;
    /* No name at all is a length of just the family; an abstract name starts with a NUL and is no path either. */
    if (length <= offsetof(struct sockaddr_un, sun_path) || path[0] == 0) return DOTNET_PAL_NOT_FOUND;
    *needed = strlen(path) + 1;
    if (*needed > capacity) return DOTNET_PAL_BUFFER_TOO_SMALL;
    memcpy(out, path, *needed); return DOTNET_PAL_OK;
}
static uint32_t local_peer_user(void *socket, uint32_t *user_id) {
    static int step;
    if (pal_local_sockets_fault == 2) switch (step++) {
    case 0: *user_id = 4242; return DOTNET_PAL_NOT_CONNECTED; /* a user written by a failing call */
    case 1: *user_id = 4242; return DOTNET_PAL_NOT_FOUND;     /* and with a status only the other calls have */
    default: *user_id = 4242; return 99u;
    }
    int fd = pal_sockets_host_descriptor(socket); struct ucred peer; socklen_t length = sizeof peer;
    if (own(fd, SO_DOMAIN) != AF_UNIX) return DOTNET_PAL_INVALID_ARGUMENT;
    /* Credentials belong to a connection. A datagram socket reports user -1 even when connected, and a listener its own user. */
    if (own(fd, SO_TYPE) != SOCK_STREAM) return DOTNET_PAL_UNSUPPORTED;
    if (own(fd, SO_ACCEPTCONN) == 1) return DOTNET_PAL_NOT_CONNECTED;
    if (getsockopt(fd, SOL_SOCKET, SO_PEERCRED, &peer, &length) != 0) return status_of(errno);
    if (peer.uid == (uid_t)-1) return DOTNET_PAL_NOT_CONNECTED;
    *user_id = peer.uid; return DOTNET_PAL_OK;
}
static const dotnet_pal_host_local_sockets table = {{DOTNET_PAL_ABI_VERSION, sizeof(dotnet_pal_host_local_sockets), DOTNET_PAL_CAP_LOCAL_SOCKETS},
    {local_bind, local_connect, local_address, local_peer_user, NULL}};
static const dotnet_pal_host_local_sockets malformed = {{DOTNET_PAL_ABI_VERSION, sizeof(dotnet_pal_host_local_sockets), DOTNET_PAL_CAP_LOCAL_SOCKETS},
    {local_bind, local_connect, local_address, NULL, NULL}};
const dotnet_pal_host_local_sockets *dotnet_pal_host_local_sockets_v2(void) { return pal_local_sockets_fault == 1 ? &malformed : &table; }

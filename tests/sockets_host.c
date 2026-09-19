/* Independent POSIX reference provider for the host-sockets conformance suite.
 * A handle is the descriptor plus one, readiness is poll(2) and every wake
 * channel is a non-blocking self-pipe. Fault 1 withholds a callback; fault 2
 * answers in the ways the front end is documented to sanitize.
 * pal_sockets_host_descriptor is for the reference providers of the network and
 * local_sockets groups, whose calls take these handles. A Unix domain socket is
 * known by asking the descriptor for its domain: the calls that take an IP
 * address refuse it, and its endpoints answer with the bare family. */
#define _GNU_SOURCE
#include "dotnet_pal.h"
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <netdb.h>
#include <netinet/in.h>
#include <netinet/tcp.h>
#include <poll.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/socket.h>
#include <sys/time.h>
#include <time.h>
#include <unistd.h>
int pal_sockets_fault;
static void *to_handle(int fd) { return (void*)(intptr_t)(fd + 1); }
static int to_fd(void *socket) { return (int)(intptr_t)socket - 1; }
int pal_sockets_host_descriptor(void *socket) { return to_fd(socket); }
/* getsockopt(IP_MULTICAST_IF) answers with an address, never with the index it was set from: the index is kept per descriptor. */
enum { TRACKED = 4096 };
static _Atomic uint32_t multicast_index[TRACKED];
static void *fresh(int fd) { if (fd < TRACKED) atomic_store(&multicast_index[fd], 0); return to_handle(fd); }
static uint32_t status_of(int code) {
    switch (code) {
    case 0: return DOTNET_PAL_OK;
    case EAGAIN: return DOTNET_PAL_WOULD_BLOCK; /* EWOULDBLOCK is the same value on Linux */
    case EINPROGRESS: return DOTNET_PAL_IN_PROGRESS;
    case EPIPE: return DOTNET_PAL_BROKEN_PIPE;
    case ECONNREFUSED: return DOTNET_PAL_CONNECTION_REFUSED;
    case ECONNRESET: return DOTNET_PAL_CONNECTION_RESET;
    case ECONNABORTED: return DOTNET_PAL_CONNECTION_ABORTED;
    case ENOTCONN: return DOTNET_PAL_NOT_CONNECTED;
    case EISCONN: return DOTNET_PAL_ALREADY_CONNECTED;
    case EADDRINUSE: return DOTNET_PAL_ADDRESS_IN_USE;
    case EADDRNOTAVAIL: return DOTNET_PAL_ADDRESS_NOT_AVAILABLE;
    case ENETUNREACH: case ENETDOWN: return DOTNET_PAL_NETWORK_UNREACHABLE;
    case EHOSTUNREACH: return DOTNET_PAL_HOST_UNREACHABLE;
    case EMFILE: case ENFILE: return DOTNET_PAL_TOO_MANY_HANDLES;
    case EMSGSIZE: return DOTNET_PAL_MESSAGE_TOO_LARGE;
    case EACCES: case EPERM: return DOTNET_PAL_ACCESS_DENIED;
    case ETIMEDOUT: return DOTNET_PAL_TIMEOUT;
    case ENOMEM: case ENOBUFS: return DOTNET_PAL_OUT_OF_MEMORY;
    case EINVAL: return DOTNET_PAL_INVALID_ARGUMENT;
    case EAFNOSUPPORT: case EPROTONOSUPPORT: case EOPNOTSUPP: case ENOPROTOOPT: return DOTNET_PAL_UNSUPPORTED;
    default: return DOTNET_PAL_OS_ERROR;
    }
}
static uint32_t result(int rc) { return rc == 0 ? DOTNET_PAL_OK : status_of(errno); }
static int own(int fd, int name) { int value = -1; socklen_t length = sizeof value; return getsockopt(fd, SOL_SOCKET, name, &value, &length) == 0 ? value : -1; }
static int is_local(int fd) { return own(fd, SO_DOMAIN) == AF_UNIX; }
/* An expired SO_RCVTIMEO or SO_SNDTIMEO is EAGAIN too; on a blocking descriptor that is the timeout. */
static uint32_t waited(int fd) {
    int code = errno;
    return code == EAGAIN && !(fcntl(fd, F_GETFL) & O_NONBLOCK) ? DOTNET_PAL_TIMEOUT : status_of(code);
}
static socklen_t to_native(const dotnet_pal_socket_address *address, struct sockaddr_storage *out) {
    memset(out, 0, sizeof *out);
    if (address->family == DOTNET_PAL_FAMILY_IPV4) {
        struct sockaddr_in v4 = {0};
        v4.sin_family = AF_INET; v4.sin_port = htons(address->port); memcpy(&v4.sin_addr, address->address, 4);
        memcpy(out, &v4, sizeof v4); return sizeof v4;
    }
    if (address->family == DOTNET_PAL_FAMILY_IPV6) {
        struct sockaddr_in6 v6 = {0};
        v6.sin6_family = AF_INET6; v6.sin6_port = htons(address->port); v6.sin6_scope_id = address->scope; memcpy(&v6.sin6_addr, address->address, 16);
        memcpy(out, &v6, sizeof v6); return sizeof v6;
    }
    return 0;
}
static int from_native(const void *native, socklen_t length, dotnet_pal_socket_address *out) {
    struct sockaddr_in v4; struct sockaddr_in6 v6; sa_family_t family;
    if (!native || length < sizeof family) return 0;
    memcpy(&family, native, sizeof family);
    memset(out, 0, sizeof *out);
    if (family == AF_INET && length >= sizeof v4) {
        memcpy(&v4, native, sizeof v4);
        out->family = DOTNET_PAL_FAMILY_IPV4; out->port = ntohs(v4.sin_port); memcpy(out->address, &v4.sin_addr, 4); return 1;
    }
    if (family == AF_INET6 && length >= sizeof v6) {
        memcpy(&v6, native, sizeof v6);
        out->family = DOTNET_PAL_FAMILY_IPV6; out->port = ntohs(v6.sin6_port); out->scope = v6.sin6_scope_id; memcpy(out->address, &v6.sin6_addr, 16); return 1;
    }
    if (family == AF_UNIX) { out->family = DOTNET_PAL_FAMILY_LOCAL; return 1; } /* the path is the local_sockets group's to report */
    return 0;
}
static uint32_t socket_create(uint32_t family, uint32_t kind, void **out) {
    if (pal_sockets_fault == 2) { *out = NULL; return DOTNET_PAL_OK; } /* success without a handle */
    int domain = family == DOTNET_PAL_FAMILY_LOCAL ? AF_UNIX : family == DOTNET_PAL_FAMILY_IPV6 ? AF_INET6 : AF_INET;
    int fd = socket(domain, (kind == DOTNET_PAL_SOCKET_DATAGRAM ? SOCK_DGRAM : SOCK_STREAM) | SOCK_CLOEXEC, 0);
    if (fd < 0) return status_of(errno);
    *out = fresh(fd); return DOTNET_PAL_OK;
}
static uint32_t socket_close(void *socket) { return close(to_fd(socket)) == 0 || errno == EINTR ? DOTNET_PAL_OK : status_of(errno); }
static uint32_t socket_bind(void *socket, const dotnet_pal_socket_address *address) {
    struct sockaddr_storage native; socklen_t length = to_native(address, &native);
    return length == 0 || is_local(to_fd(socket)) ? DOTNET_PAL_INVALID_ARGUMENT : result(bind(to_fd(socket), (struct sockaddr*)&native, length));
}
static uint32_t socket_listen(void *socket, uint32_t backlog) { return result(listen(to_fd(socket), backlog > INT_MAX ? INT_MAX : (int)backlog)); }
static uint32_t socket_accept(void *socket, void **out, dotnet_pal_socket_address *peer) {
    struct sockaddr_storage native; socklen_t length = sizeof native; int fd;
    do fd = accept(to_fd(socket), (struct sockaddr*)&native, &length); while (fd < 0 && errno == EINTR);
    if (fd < 0) return waited(to_fd(socket));
    (void)fcntl(fd, F_SETFD, FD_CLOEXEC);
    from_native(&native, length, peer);
    if (pal_sockets_fault == 2) peer->family = 9; /* a family the boundary does not define */
    *out = fresh(fd); return DOTNET_PAL_OK;
}
static uint32_t pending_error(int fd, int *code) {
    socklen_t length = sizeof *code; *code = 0;
    return result(getsockopt(fd, SOL_SOCKET, SO_ERROR, code, &length));
}
static uint32_t socket_connect(void *socket, const dotnet_pal_socket_address *address) {
    struct sockaddr_storage native; socklen_t length = to_native(address, &native); int fd = to_fd(socket), code;
    if (length == 0 || is_local(fd)) return DOTNET_PAL_INVALID_ARGUMENT;
    if (connect(fd, (struct sockaddr*)&native, length) == 0) return DOTNET_PAL_OK;
    if (errno != EINTR) return status_of(errno);
    /* The interrupted attempt is still running: its result arrives as writability. */
    struct pollfd wait = {fd, POLLOUT, 0};
    while (poll(&wait, 1, -1) < 0) if (errno != EINTR) return status_of(errno);
    uint32_t status = pending_error(fd, &code);
    return status != DOTNET_PAL_OK ? status : status_of(code);
}
static uint32_t socket_send(void *socket, const uint8_t *data, size_t size, const dotnet_pal_socket_address *to, size_t *sent) {
    if (pal_sockets_fault == 2) { *sent = size + 1; return DOTNET_PAL_OK; } /* more than was offered */
    struct sockaddr_storage native; socklen_t length = to ? to_native(to, &native) : 0; ssize_t n;
    if (to && (length == 0 || is_local(to_fd(socket)))) return DOTNET_PAL_INVALID_ARGUMENT;
    do n = to ? sendto(to_fd(socket), data, size, MSG_NOSIGNAL, (struct sockaddr*)&native, length) : send(to_fd(socket), data, size, MSG_NOSIGNAL);
    while (n < 0 && errno == EINTR);
    if (n < 0) return waited(to_fd(socket));
    *sent = (size_t)n; return DOTNET_PAL_OK;
}
static uint32_t socket_receive(void *socket, uint8_t *data, size_t capacity, uint32_t flags, dotnet_pal_socket_address *from, size_t *received) {
    if (pal_sockets_fault == 2) { *received = capacity + 1; return DOTNET_PAL_OK; } /* more than fits */
    struct sockaddr_storage native; socklen_t length; ssize_t n;
    do { length = sizeof native; n = recvfrom(to_fd(socket), data, capacity, (flags & DOTNET_PAL_RECEIVE_PEEK) ? MSG_PEEK : 0, (struct sockaddr*)&native, &length); }
    while (n < 0 && errno == EINTR);
    if (n < 0) return waited(to_fd(socket));
    if (from) from_native(&native, length, from); /* a stream leaves the caller's zero address */
    /* The kernel names a local sender by whether it has a path, stream or not. A local stream names none, like any stream, and the
     * sender of a local datagram is a local socket, whatever its name. */
    if (from && is_local(to_fd(socket))) { memset(from, 0, sizeof *from); if (own(to_fd(socket), SO_TYPE) == SOCK_DGRAM) from->family = DOTNET_PAL_FAMILY_LOCAL; }
    *received = (size_t)n; return DOTNET_PAL_OK;
}
static uint32_t socket_shutdown(void *socket, uint32_t how) {
    return result(shutdown(to_fd(socket), how == DOTNET_PAL_SHUTDOWN_READ ? SHUT_RD : how == DOTNET_PAL_SHUTDOWN_WRITE ? SHUT_WR : SHUT_RDWR));
}
static uint32_t socket_local(void *socket, dotnet_pal_socket_address *out) {
    struct sockaddr_storage native; socklen_t length = sizeof native;
    if (getsockname(to_fd(socket), (struct sockaddr*)&native, &length) != 0) return status_of(errno);
    if (!from_native(&native, length, out)) return DOTNET_PAL_OS_ERROR;
    if (pal_sockets_fault == 2) out->family = 0; /* success without an address */
    return DOTNET_PAL_OK;
}
static uint32_t socket_peer(void *socket, dotnet_pal_socket_address *out) {
    struct sockaddr_storage native; socklen_t length = sizeof native;
    if (getpeername(to_fd(socket), (struct sockaddr*)&native, &length) != 0) return status_of(errno);
    return from_native(&native, length, out) ? DOTNET_PAL_OK : DOTNET_PAL_OS_ERROR;
}
static uint32_t socket_blocking(void *socket, uint32_t blocking) {
    int flags = fcntl(to_fd(socket), F_GETFL);
    if (flags < 0) return status_of(errno);
    return result(fcntl(to_fd(socket), F_SETFL, blocking ? flags & ~O_NONBLOCK : flags | O_NONBLOCK));
}
static int plain_option(uint32_t option, int *level, int *name) {
    switch (option) {
    case DOTNET_PAL_SOCKET_REUSE_ADDRESS: *level = SOL_SOCKET; *name = SO_REUSEADDR; return 1;
    case DOTNET_PAL_SOCKET_NO_DELAY: *level = IPPROTO_TCP; *name = TCP_NODELAY; return 1;
    case DOTNET_PAL_SOCKET_KEEP_ALIVE: *level = SOL_SOCKET; *name = SO_KEEPALIVE; return 1;
    case DOTNET_PAL_SOCKET_BROADCAST: *level = SOL_SOCKET; *name = SO_BROADCAST; return 1;
    case DOTNET_PAL_SOCKET_RECEIVE_BUFFER: *level = SOL_SOCKET; *name = SO_RCVBUF; return 1;
    case DOTNET_PAL_SOCKET_SEND_BUFFER: *level = SOL_SOCKET; *name = SO_SNDBUF; return 1;
    case DOTNET_PAL_SOCKET_IPV6_ONLY: *level = IPPROTO_IPV6; *name = IPV6_V6ONLY; return 1;
    default: return 0;
    }
}
/* The hop limits and the multicast options live at the level of the socket's family; 0 for an option that is none of them. */
static int family_option(uint32_t option, int v6, int *level, int *name) {
    *level = v6 ? IPPROTO_IPV6 : IPPROTO_IP;
    switch (option) {
    case DOTNET_PAL_SOCKET_HOPS: *name = v6 ? IPV6_UNICAST_HOPS : IP_TTL; return 1;
    case DOTNET_PAL_SOCKET_MULTICAST_HOPS: *name = v6 ? IPV6_MULTICAST_HOPS : IP_MULTICAST_TTL; return 1;
    case DOTNET_PAL_SOCKET_MULTICAST_LOOPBACK: *name = v6 ? IPV6_MULTICAST_LOOP : IP_MULTICAST_LOOP; return 1;
    case DOTNET_PAL_SOCKET_MULTICAST_INTERFACE: *name = v6 ? IPV6_MULTICAST_IF : IP_MULTICAST_IF; return 1;
    default: return 0;
    }
}
static int keep_alive_option(uint32_t option) {
    return option == DOTNET_PAL_SOCKET_KEEP_ALIVE_IDLE ? TCP_KEEPIDLE : option == DOTNET_PAL_SOCKET_KEEP_ALIVE_INTERVAL ? TCP_KEEPINTVL
        : option == DOTNET_PAL_SOCKET_KEEP_ALIVE_COUNT ? TCP_KEEPCNT : 0;
}
/* Whether the socket's protocol has the option at all, asked of the socket rather than left to the error a kernel happens to choose. */
static uint32_t shape(int fd, uint32_t option, int *v6) {
    int type = 0, domain = 0; socklen_t length = sizeof type;
    if (getsockopt(fd, SOL_SOCKET, SO_TYPE, &type, &length) != 0) return status_of(errno);
    length = sizeof domain;
    if (getsockopt(fd, SOL_SOCKET, SO_DOMAIN, &domain, &length) != 0) return status_of(errno);
    *v6 = domain == AF_INET6;
    if (domain == AF_UNIX) return DOTNET_PAL_UNSUPPORTED; /* neither TCP tuning nor hop limits nor multicast */
    if (keep_alive_option(option) && type != SOCK_STREAM) return DOTNET_PAL_UNSUPPORTED;
    if (option >= DOTNET_PAL_SOCKET_MULTICAST_HOPS && option <= DOTNET_PAL_SOCKET_MULTICAST_INTERFACE && type != SOCK_DGRAM) return DOTNET_PAL_UNSUPPORTED;
    return DOTNET_PAL_OK;
}
static uint32_t socket_get_option(void *socket, uint32_t option, uint64_t *value) {
    int fd = to_fd(socket), level, name, number = 0, v6 = 0; socklen_t length = sizeof number;
    if (option >= DOTNET_PAL_SOCKET_KEEP_ALIVE_IDLE && option <= DOTNET_PAL_SOCKET_MULTICAST_INTERFACE) {
        uint32_t status = shape(fd, option, &v6);
        if (status != DOTNET_PAL_OK) return status;
        if (option == DOTNET_PAL_SOCKET_MULTICAST_INTERFACE && !v6) {
            if (fd >= TRACKED) return DOTNET_PAL_UNSUPPORTED;
            *value = atomic_load(&multicast_index[fd]); return DOTNET_PAL_OK;
        }
        if (keep_alive_option(option)) { level = IPPROTO_TCP; name = keep_alive_option(option); } else family_option(option, v6, &level, &name);
        if (getsockopt(fd, level, name, &number, &length) != 0) return status_of(errno);
        *value = option == DOTNET_PAL_SOCKET_MULTICAST_LOOPBACK ? (uint64_t)(number != 0) : number < 0 ? 0 : (uint64_t)number;
        return DOTNET_PAL_OK;
    }
    if (plain_option(option, &level, &name)) {
        if (getsockopt(fd, level, name, &number, &length) != 0) return status_of(errno);
        *value = option == DOTNET_PAL_SOCKET_RECEIVE_BUFFER || option == DOTNET_PAL_SOCKET_SEND_BUFFER ? (uint64_t)number : (uint64_t)(number != 0);
        return DOTNET_PAL_OK;
    }
    if (option == DOTNET_PAL_SOCKET_LINGER) {
        struct linger linger = {0, 0}; length = sizeof linger;
        if (getsockopt(fd, SOL_SOCKET, SO_LINGER, &linger, &length) != 0) return status_of(errno);
        *value = linger.l_onoff ? (uint64_t)linger.l_linger + 1 : 0; return DOTNET_PAL_OK;
    }
    if (option == DOTNET_PAL_SOCKET_RECEIVE_TIMEOUT || option == DOTNET_PAL_SOCKET_SEND_TIMEOUT) {
        struct timeval limit = {0, 0}; length = sizeof limit;
        if (getsockopt(fd, SOL_SOCKET, option == DOTNET_PAL_SOCKET_RECEIVE_TIMEOUT ? SO_RCVTIMEO : SO_SNDTIMEO, &limit, &length) != 0) return status_of(errno);
        *value = (uint64_t)limit.tv_sec * 1000 + ((uint64_t)limit.tv_usec + 999) / 1000; return DOTNET_PAL_OK;
    }
    if (option == DOTNET_PAL_SOCKET_ERROR) {
        static int calls;
        /* Neither value is a status: one is small, the other hides a real status in its low half. */
        if (pal_sockets_fault == 2) { *value = calls++ % 2 ? UINT64_C(0x100000000) | DOTNET_PAL_CONNECTION_REFUSED : 99; return DOTNET_PAL_OK; }
        uint32_t status = pending_error(fd, &number);
        if (status == DOTNET_PAL_OK) *value = status_of(number);
        return status;
    }
    if (option == DOTNET_PAL_SOCKET_AVAILABLE) {
        if (ioctl(fd, FIONREAD, &number) != 0) return status_of(errno);
        *value = (uint64_t)number; return DOTNET_PAL_OK;
    }
    return DOTNET_PAL_INVALID_ARGUMENT;
}
static uint32_t socket_set_option(void *socket, uint32_t option, uint64_t value) {
    int fd = to_fd(socket), level, name, number = value > INT_MAX ? INT_MAX : (int)value, v6 = 0;
    if (option >= DOTNET_PAL_SOCKET_KEEP_ALIVE_IDLE && option <= DOTNET_PAL_SOCKET_MULTICAST_INTERFACE) {
        uint32_t status = shape(fd, option, &v6);
        if (status != DOTNET_PAL_OK) return status;
        if (option == DOTNET_PAL_SOCKET_MULTICAST_INTERFACE && !v6) {
            struct ip_mreqn by_index = {{0}, {0}, number};
            if (fd >= TRACKED) return DOTNET_PAL_UNSUPPORTED;
            if (setsockopt(fd, IPPROTO_IP, IP_MULTICAST_IF, &by_index, sizeof by_index) != 0) return status_of(errno);
            atomic_store(&multicast_index[fd], (uint32_t)number); return DOTNET_PAL_OK;
        }
        if (keep_alive_option(option)) { level = IPPROTO_TCP; name = keep_alive_option(option); } else family_option(option, v6, &level, &name);
        if (setsockopt(fd, level, name, &number, sizeof number) == 0) return DOTNET_PAL_OK;
        /* An interface that does not exist is EADDRNOTAVAIL from IPv4 and ENODEV from IPv6. */
        return option == DOTNET_PAL_SOCKET_MULTICAST_INTERFACE && errno == ENODEV ? DOTNET_PAL_ADDRESS_NOT_AVAILABLE : status_of(errno);
    }
    if (plain_option(option, &level, &name)) return result(setsockopt(fd, level, name, &number, sizeof number));
    if (option == DOTNET_PAL_SOCKET_LINGER) {
        struct linger linger = {value != 0, value > 1 ? (value - 1 > INT_MAX ? INT_MAX : (int)(value - 1)) : 0};
        return result(setsockopt(fd, SOL_SOCKET, SO_LINGER, &linger, sizeof linger));
    }
    if (option == DOTNET_PAL_SOCKET_RECEIVE_TIMEOUT || option == DOTNET_PAL_SOCKET_SEND_TIMEOUT) {
        struct timeval limit = {(time_t)(value / 1000), (suseconds_t)(value % 1000 * 1000)};
        return result(setsockopt(fd, SOL_SOCKET, option == DOTNET_PAL_SOCKET_RECEIVE_TIMEOUT ? SO_RCVTIMEO : SO_SNDTIMEO, &limit, sizeof limit));
    }
    return DOTNET_PAL_INVALID_ARGUMENT;
}
static pthread_mutex_t wake_lock = PTHREAD_MUTEX_INITIALIZER;
static int wake_pipes[DOTNET_PAL_POLL_CHANNELS][2], wake_made[DOTNET_PAL_POLL_CHANNELS];
/* One end of a channel's pipe, made on first use: 0 is polled, 1 is written. */
static int wake_end(uint32_t channel, int end) {
    int fd = -1;
    pthread_mutex_lock(&wake_lock);
    if (wake_made[channel] || pipe2(wake_pipes[channel], O_NONBLOCK | O_CLOEXEC) == 0) { wake_made[channel] = 1; fd = wake_pipes[channel][end]; }
    pthread_mutex_unlock(&wake_lock);
    return fd;
}
static uint64_t now(void) {
    struct timespec ts; clock_gettime(CLOCK_MONOTONIC, &ts);
    return (uint64_t)ts.tv_sec * UINT64_C(1000000000) + (uint64_t)ts.tv_nsec;
}
static uint32_t socket_poll(dotnet_pal_poll_entry *entries, size_t count, uint64_t timeout_ns, uint32_t channel, size_t *ready) {
    if (channel != DOTNET_PAL_NO_CHANNEL && channel >= DOTNET_PAL_POLL_CHANNELS) return DOTNET_PAL_INVALID_ARGUMENT;
    /* Without a channel the last slot holds -1, which poll ignores. */
    int wake = channel == DOTNET_PAL_NO_CHANNEL ? -1 : wake_end(channel, 0);
    if (channel != DOTNET_PAL_NO_CHANNEL && wake < 0) return DOTNET_PAL_OS_ERROR;
    struct pollfd *set = calloc(count + 1, sizeof *set);
    if (!set) return DOTNET_PAL_OUT_OF_MEMORY;
    for (size_t i = 0; i < count; ++i) {
        set[i].fd = to_fd(entries[i].socket);
        set[i].events = (short)((entries[i].requested & DOTNET_PAL_POLL_READ ? POLLIN | POLLRDHUP : 0) | (entries[i].requested & DOTNET_PAL_POLL_WRITE ? POLLOUT : 0));
    }
    set[count].fd = wake; set[count].events = POLLIN;
    uint64_t start = now(), deadline = timeout_ns > UINT64_MAX - start ? UINT64_MAX : start + timeout_ns;
    int rc;
    for (;;) {
        /* poll counts milliseconds: round the remainder up and go around again when it was clamped or interrupted. */
        uint64_t current = now(), left = current < deadline ? deadline - current : 0, ms = left / 1000000 + (left % 1000000 != 0);
        rc = poll(set, (nfds_t)count + 1, timeout_ns == DOTNET_PAL_INFINITE_NS ? -1 : ms > INT_MAX ? INT_MAX : (int)ms);
        if (rc > 0 || (rc == 0 && now() >= deadline)) break;
        if (rc < 0 && errno != EINTR) { int code = errno; free(set); return status_of(code); }
    }
    *ready = 0;
    for (size_t i = 0; i < count; ++i) {
        short got = set[i].revents;
        entries[i].triggered = (got & POLLIN ? DOTNET_PAL_POLL_READ : 0) | (got & POLLOUT ? DOTNET_PAL_POLL_WRITE : 0)
            | (got & (POLLERR | POLLNVAL) ? DOTNET_PAL_POLL_ERROR : 0) | (got & (POLLHUP | POLLRDHUP) ? DOTNET_PAL_POLL_HANGUP : 0);
        if (pal_sockets_fault == 2) entries[i].triggered = UINT32_MAX; /* undefined bits and bits nobody asked for */
        *ready += entries[i].triggered != 0;
    }
    if (pal_sockets_fault == 2) *ready = count + 7;
    if (set[count].revents & POLLIN) { uint8_t drain[64]; while (read(wake, drain, sizeof drain) > 0) {} }
    free(set);
    return DOTNET_PAL_OK;
}
static uint32_t socket_wake(uint32_t channel) {
    if (channel >= DOTNET_PAL_POLL_CHANNELS) return DOTNET_PAL_INVALID_ARGUMENT;
    int fd = wake_end(channel, 1);
    if (fd < 0) return DOTNET_PAL_OS_ERROR;
    for (;;) {
        if (write(fd, "w", 1) == 1 || errno == EAGAIN) return DOTNET_PAL_OK; /* a full pipe is already signaled */
        if (errno != EINTR) return status_of(errno);
    }
}
static uint32_t socket_resolve(const uint8_t *name, size_t name_length, uint32_t family, dotnet_pal_socket_address *out, size_t capacity, size_t *count) {
    if (pal_sockets_fault == 2) { out[0].family = 9; *count = capacity + 1; return DOTNET_PAL_OK; } /* more answers than room */
    char node[256]; struct addrinfo hints = {0}, *list = NULL;
    if (name_length >= sizeof node) return DOTNET_PAL_INVALID_ARGUMENT;
    memcpy(node, name, name_length); node[name_length] = 0;
    hints.ai_family = family == DOTNET_PAL_FAMILY_IPV4 ? AF_INET : family == DOTNET_PAL_FAMILY_IPV6 ? AF_INET6 : AF_UNSPEC;
    hints.ai_socktype = SOCK_STREAM;
    int rc = getaddrinfo(node, NULL, &hints, &list);
    if (rc == EAI_NONAME || rc == EAI_NODATA) return DOTNET_PAL_NOT_FOUND;
    if (rc == EAI_AGAIN) return DOTNET_PAL_TIMEOUT;
    if (rc != 0) return rc == EAI_MEMORY ? DOTNET_PAL_OUT_OF_MEMORY : rc == EAI_SYSTEM ? status_of(errno) : DOTNET_PAL_OS_ERROR;
    size_t found = 0;
    for (struct addrinfo *info = list; info && found < capacity; info = info->ai_next) found += (size_t)from_native(info->ai_addr, info->ai_addrlen, &out[found]);
    freeaddrinfo(list);
    *count = found;
    return found ? DOTNET_PAL_OK : DOTNET_PAL_NOT_FOUND;
}
static uint32_t socket_host_name(uint8_t *out, size_t capacity, size_t *needed) {
    char name[257] = {0};
    if (gethostname(name, sizeof name - 1) != 0) return status_of(errno);
    *needed = strlen(name) + 1;
    if (*needed > capacity) return DOTNET_PAL_BUFFER_TOO_SMALL;
    memcpy(out, name, *needed); return DOTNET_PAL_OK;
}
#define CALLBACKS(poll_callback) {socket_create, socket_close, socket_bind, socket_listen, socket_accept, socket_connect, socket_send, socket_receive, \
    socket_shutdown, socket_local, socket_peer, socket_blocking, socket_get_option, socket_set_option, poll_callback, socket_wake, socket_resolve, socket_host_name, NULL}
static const dotnet_pal_host_sockets table = {{DOTNET_PAL_ABI_VERSION, sizeof(dotnet_pal_host_sockets), DOTNET_PAL_CAP_SOCKETS}, CALLBACKS(socket_poll)};
static const dotnet_pal_host_sockets malformed = {{DOTNET_PAL_ABI_VERSION, sizeof(dotnet_pal_host_sockets), DOTNET_PAL_CAP_SOCKETS}, CALLBACKS(NULL)};
const dotnet_pal_host_sockets *dotnet_pal_host_sockets_v2(void) { return pal_sockets_fault == 1 ? &malformed : &table; }

/* Conformance test of the local_sockets group on Linux: Unix domain sockets made
 * by the sockets group, bound and connected by path inside one process, with the
 * paths and the peer's user compared with what the kernel says about the same
 * descriptors, the failures of bind and connect, argument checks and counters.
 * With PAL_HOST_TEST the same program runs against the C host tables; faults 1
 * and 2 check rejection and sanitizing. */
#define _GNU_SOURCE
#include "dotnet_pal.h"
#include <assert.h>
#include <errno.h>
#include <fcntl.h>
#include <grp.h>
#include <limits.h>
#include <signal.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mount.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/un.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>
#ifdef PAL_HOST_TEST
extern int pal_local_sockets_fault;
#endif
enum { V4 = DOTNET_PAL_FAMILY_IPV4, LOCAL = DOTNET_PAL_FAMILY_LOCAL, TCP = DOTNET_PAL_SOCKET_STREAM, UDP = DOTNET_PAL_SOCKET_DATAGRAM,
    READ = DOTNET_PAL_POLL_READ, WRITE = DOTNET_PAL_POLL_WRITE, HANGUP = DOTNET_PAL_POLL_HANGUP };
#define INVALID DOTNET_PAL_INVALID_ARGUMENT
#define SMALL DOTNET_PAL_BUFFER_TOO_SMALL
#define MISSING DOTNET_PAL_NOT_FOUND
#define REFUSED DOTNET_PAL_CONNECTION_REFUSED
#define CAPACITY (DOTNET_PAL_MAX_NAME + 1)
#define MS UINT64_C(1000000)
#define TEXT(literal) ((const uint8_t*)(literal))
static const dotnet_pal_sockets_ops *s;
static const dotnet_pal_local_sockets_ops *l;
static uint8_t out[CAPACITY + 1];
static char root[64];
/* What the counters have to say at the end: every call of the group goes through these four. */
static dotnet_pal_local_sockets_stats expected;
static uint32_t tally(uint32_t status, uint64_t *ok) { ++*(status == DOTNET_PAL_OK ? ok : &expected.rejected_or_failed); return status; }
static uint32_t l_bind(void *socket, const char *path) { return tally(l->bind(socket, TEXT(path), strlen(path)), &expected.bind_ok); }
static uint32_t l_connect(void *socket, const char *path) { return tally(l->connect(socket, TEXT(path), strlen(path)), &expected.connect_ok); }
static uint32_t l_address(void *socket, uint32_t peer, uint8_t *buffer, size_t capacity, size_t *needed) { return tally(l->address(socket, peer, buffer, capacity, needed), &expected.address_ok); }
static uint32_t l_user(void *socket, uint32_t *user) { return tally(l->peer_user(socket, user), &expected.peer_ok); }
static int zero(const uint8_t *bytes, size_t size) { while (size--) if (*bytes++) return 0; return 1; }
static uint64_t now(void) { struct timespec ts; assert(clock_gettime(CLOCK_MONOTONIC, &ts) == 0); return (uint64_t)ts.tv_sec * UINT64_C(1000000000) + (uint64_t)ts.tv_nsec; }
/* A path inside the scratch directory; eight may be in use at once. */
static const char *in(const char *name) {
    static char paths[8][128]; static unsigned next;
    char *path = paths[next++ % 8];
    assert((size_t)snprintf(path, sizeof paths[0], "%s/%s", root, name) < sizeof paths[0]);
    return path;
}
/* The descriptor the provider's next socket gets: the kernel hands out the lowest free one, and no other thread of this program is running. */
static int next_descriptor(void) { int fd = dup(0); assert(fd >= 0 && close(fd) == 0 && fcntl(fd, F_GETFD) == -1 && errno == EBADF); return fd; }
static int own(int fd, int name) { int value = -7; socklen_t length = sizeof value; assert(getsockopt(fd, SOL_SOCKET, name, &value, &length) == 0); return value; }
/* A local socket and its descriptor, which is a close-on-exec Unix domain socket of the kind asked for. */
static void *watched(uint32_t kind, int *fd) {
    void *socket = NULL;
    *fd = next_descriptor();
    assert(s->create(LOCAL, kind, &socket) == 0 && socket);
    assert(own(*fd, SO_DOMAIN) == AF_UNIX && own(*fd, SO_TYPE) == (kind == TCP ? SOCK_STREAM : SOCK_DGRAM) && (fcntl(*fd, F_GETFD) & FD_CLOEXEC));
    return socket;
}
static void *open_socket(uint32_t family, uint32_t kind) { void *socket = NULL; assert(s->create(family, kind, &socket) == 0 && socket); return socket; }
static void closed(void *socket, int fd) { assert(s->close(socket) == 0 && fcntl(fd, F_GETFD) == -1 && errno == EBADF); }
/* What the kernel itself reports for a descriptor: the path, or "" for a socket without a name. */
static const char *kernel_name(int fd, int peer) {
    static struct sockaddr_storage native; socklen_t length = sizeof native;
    memset(&native, 0, sizeof native);
    assert((peer ? getpeername : getsockname)(fd, (struct sockaddr*)&native, &length) == 0 && native.ss_family == AF_UNIX);
    return length <= offsetof(struct sockaddr_un, sun_path) ? "" : ((struct sockaddr_un*)&native)->sun_path;
}
static uint32_t kernel_user(int fd) { struct ucred peer; socklen_t length = sizeof peer; assert(getsockopt(fd, SOL_SOCKET, SO_PEERCRED, &peer, &length) == 0); return peer.uid; }
/* The text contract of address: the path with its NUL when it fits, the length it needs either way, and a short buffer cleared. */
static void answers(void *socket, uint32_t peer, const char *path) {
    size_t length = strlen(path) + 1, needed = 7;
    memset(out, 0xAA, sizeof out);
    assert(l_address(socket, peer, out, CAPACITY, &needed) == 0 && needed == length && strcmp((char*)out, path) == 0 && zero(out + length, CAPACITY - length) && out[CAPACITY] == 0xAA);
    memset(out, 0xAA, sizeof out); needed = 7;
    assert(l_address(socket, peer, out, length, &needed) == 0 && needed == length && strcmp((char*)out, path) == 0 && out[length] == 0xAA);
    memset(out, 0xAA, sizeof out); needed = 7;
    assert(l_address(socket, peer, out, length - 1, &needed) == SMALL && needed == length && zero(out, length - 1) && out[length - 1] == 0xAA);
    needed = 7;
    assert(l_address(socket, peer, NULL, 0, &needed) == SMALL && needed == length);
}
static void refuses(void *socket, uint32_t peer, uint32_t status) {
    size_t needed = 7;
    memset(out, 0xAA, sizeof out);
    assert(l_address(socket, peer, out, CAPACITY, &needed) == status && needed == 0 && zero(out, CAPACITY));
}
static void no_user(void *socket, uint32_t status) { uint32_t user = 7; assert(l_user(socket, &user) == status && user == UINT32_MAX); }
static int bare(const dotnet_pal_socket_address *address) { dotnet_pal_socket_address local = {LOCAL, 0, 0, {0}}; return memcmp(address, &local, sizeof local) == 0; }
static int is_socket(const char *path) { struct stat node; return lstat(path, &node) == 0 && S_ISSOCK(node.st_mode); }
static uint32_t bits(void *socket, uint32_t requested, uint64_t timeout_ns) {
    dotnet_pal_poll_entry entry = {socket, requested, 99}; size_t ready = 7;
    assert(s->poll(&entry, 1, timeout_ns, DOTNET_PAL_NO_CHANNEL, &ready) == 0 && ready == (entry.triggered != 0));
    return entry.triggered;
}
static uint64_t option(void *socket, uint32_t name) { uint64_t value = 7; assert(s->get_option(socket, name, &value) == 0); return value; }
static void unsupported(void *socket, uint32_t name) {
    uint64_t value = 7;
    assert(s->get_option(socket, name, &value) == DOTNET_PAL_UNSUPPORTED && value == 0 && s->set_option(socket, name, 1) == DOTNET_PAL_UNSUPPORTED);
}
/* Sends `text` one way and expects exactly it on the blocking other side; a stream names no sender. */
static void transfer(void *from, void *to, const char *text) {
    size_t length = strlen(text), done = 7, total = 0; uint8_t buffer[64]; dotnet_pal_socket_address sender;
    assert(length <= sizeof buffer && s->send(from, TEXT(text), length, NULL, &done) == 0 && done == length);
    while (total < length) {
        memset(&sender, 0xff, sizeof sender);
        assert(s->receive(to, buffer + total, sizeof buffer - total, 0, &sender, &done) == 0 && done > 0 && sender.family == 0 && sender.port == 0);
        total += done;
    }
    assert(memcmp(buffer, text, length) == 0);
}
static void *listening(const char *path, uint32_t backlog, int *fd) {
    void *server = watched(TCP, fd);
    assert(l_bind(server, path) == 0 && s->listen(server, backlog) == 0);
    return server;
}

/* A listener on a path, a client that connects to it, and what each end says about itself and the other. */
static void stream(void) {
    int server_fd, client_fd, accepted_fd, named_fd; dotnet_pal_socket_address address, peer; uint32_t user = 7; uint8_t buffer[16]; size_t done = 7;
    const char *path = in("listener"), *second = in("second"), *client_path = in("client");
    void *server = watched(TCP, &server_fd);
    /* Before bind: no path, no peer, and the bare family as the address. */
    refuses(server, 0, MISSING); refuses(server, 1, DOTNET_PAL_NOT_CONNECTED); no_user(server, DOTNET_PAL_NOT_CONNECTED);
    assert(s->local_address(server, &address) == 0 && bare(&address));
    memset(&address, 0xff, sizeof address);
    assert(s->peer_address(server, &address) == DOTNET_PAL_NOT_CONNECTED && address.family == 0);
    assert(l_bind(server, path) == 0 && is_socket(path) && strcmp(kernel_name(server_fd, 0), path) == 0);
    answers(server, 0, path);
    /* A socket has one name: the kernel would call its own path taken, and must not be left to create a second one. */
    assert(l_bind(server, path) == INVALID && l_bind(server, second) == INVALID && access(second, F_OK) != 0 && errno == ENOENT);
    assert(s->listen(server, 8) == 0);
    /* The kernel answers a listener's SO_PEERCRED with the listener's own user; a listener has no peer. */
    assert(kernel_user(server_fd) == geteuid());
    no_user(server, DOTNET_PAL_NOT_CONNECTED); refuses(server, 1, DOTNET_PAL_NOT_CONNECTED);

    void *client = watched(TCP, &client_fd), *accepted = NULL;
    assert(l_connect(client, path) == 0);
    accepted_fd = next_descriptor();
    memset(&peer, 0xff, sizeof peer);
    assert(s->accept(server, &accepted, &peer) == 0 && accepted && bare(&peer) && own(accepted_fd, SO_DOMAIN) == AF_UNIX && (fcntl(accepted_fd, F_GETFD) & FD_CLOEXEC));
    void *ends[3] = {server, client, accepted};
    for (int i = 0; i < 3; ++i) {
        memset(&address, 0xff, sizeof address);
        assert(s->local_address(ends[i], &address) == 0 && bare(&address));
        if (i) { memset(&address, 0xff, sizeof address); assert(s->peer_address(ends[i], &address) == 0 && bare(&address)); }
    }
    /* The client has no name of its own; each end names the listener's path where the kernel does. */
    assert(strcmp(kernel_name(client_fd, 0), "") == 0 && strcmp(kernel_name(client_fd, 1), path) == 0 && strcmp(kernel_name(accepted_fd, 0), path) == 0 && strcmp(kernel_name(accepted_fd, 1), "") == 0);
    refuses(client, 0, MISSING); answers(client, 1, path); answers(accepted, 0, path); refuses(accepted, 1, MISSING);
    assert(l_user(client, &user) == 0 && user == geteuid() && user == kernel_user(client_fd));
    user = 7;
    assert(l_user(accepted, &user) == 0 && user == geteuid() && user == kernel_user(accepted_fd));
    transfer(client, accepted, "ping"); transfer(accepted, client, "pong!");
    /* The rest of the sockets group works as on any stream: peeking, the bytes that wait, a receive timeout, a half close. */
    assert(s->send(accepted, TEXT("again"), 5, NULL, &done) == 0 && done == 5);
    assert(s->receive(client, buffer, sizeof buffer, DOTNET_PAL_RECEIVE_PEEK, NULL, &done) == 0 && done == 5 && option(client, DOTNET_PAL_SOCKET_AVAILABLE) == 5);
    assert(s->receive(client, buffer, sizeof buffer, 0, NULL, &done) == 0 && done == 5 && memcmp(buffer, "again", 5) == 0 && option(client, DOTNET_PAL_SOCKET_AVAILABLE) == 0);
    assert(s->set_option(client, DOTNET_PAL_SOCKET_RECEIVE_TIMEOUT, 100) == 0);
    uint64_t start = now();
    assert(s->receive(client, buffer, sizeof buffer, 0, NULL, &done) == DOTNET_PAL_TIMEOUT && done == 0 && now() - start >= 80 * MS);
    assert(s->set_option(client, DOTNET_PAL_SOCKET_RECEIVE_TIMEOUT, 0) == 0 && option(client, DOTNET_PAL_SOCKET_ERROR) == DOTNET_PAL_OK && option(client, DOTNET_PAL_SOCKET_RECEIVE_BUFFER) != 0);
    assert(l_connect(client, path) == DOTNET_PAL_ALREADY_CONNECTED);
    assert(s->shutdown(client, DOTNET_PAL_SHUTDOWN_WRITE) == 0 && s->receive(accepted, buffer, sizeof buffer, 0, NULL, &done) == 0 && done == 0);
    assert(bits(accepted, READ, 0) == (READ | HANGUP) && bits(accepted, WRITE, 0) == WRITE);
    transfer(accepted, client, "late");
    assert(s->send(client, TEXT("x"), 1, NULL, &done) == DOTNET_PAL_BROKEN_PIPE && done == 0);
    /* What a local socket's protocol does not have: TCP tuning, hop limits, the IPv6 switch. */
    static const uint32_t missing[] = {DOTNET_PAL_SOCKET_NO_DELAY, DOTNET_PAL_SOCKET_IPV6_ONLY, DOTNET_PAL_SOCKET_KEEP_ALIVE_IDLE, DOTNET_PAL_SOCKET_KEEP_ALIVE_INTERVAL,
        DOTNET_PAL_SOCKET_KEEP_ALIVE_COUNT, DOTNET_PAL_SOCKET_HOPS, DOTNET_PAL_SOCKET_MULTICAST_HOPS, DOTNET_PAL_SOCKET_MULTICAST_LOOPBACK, DOTNET_PAL_SOCKET_MULTICAST_INTERFACE};
    for (size_t i = 0; i < sizeof missing / sizeof *missing; ++i) unsupported(client, missing[i]);
    closed(client, client_fd); closed(accepted, accepted_fd);

    /* A client with a path of its own: the accepted end names it as its peer, and the path may be relative. */
    void *named = watched(TCP, &named_fd);
    assert(l_bind(named, client_path) == 0 && l_connect(named, path) == 0);
    accepted_fd = next_descriptor();
    assert(s->accept(server, &accepted, &peer) == 0 && bare(&peer) && strcmp(kernel_name(accepted_fd, 1), client_path) == 0);
    answers(named, 0, client_path); answers(named, 1, path); answers(accepted, 1, client_path);
    /* The peer's user is the one it connected as: it outlives the peer, as does the peer's path. */
    closed(named, named_fd);
    user = 7;
    assert(l_user(accepted, &user) == 0 && user == geteuid());
    answers(accepted, 1, client_path);
    closed(accepted, accepted_fd);
    void *relative = watched(TCP, &named_fd);
    assert(l_bind(relative, "relative") == 0 && is_socket(in("relative")) && strcmp(kernel_name(named_fd, 0), "relative") == 0);
    answers(relative, 0, "relative");
    closed(relative, named_fd);

    /* Peers the group did not make: an abstract name is no path, and a path that fills the kernel's field arrives whole. */
    struct sockaddr_un target, name; int raw = socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0); char longest[sizeof name.sun_path + 1];
    memset(&target, 0, sizeof target); target.sun_family = AF_UNIX; strcpy(target.sun_path, path);
    memset(&name, 0, sizeof name); name.sun_family = AF_UNIX; snprintf(name.sun_path + 1, sizeof name.sun_path - 1, "pal-local-%ld", (long)getpid());
    assert(raw >= 0 && bind(raw, (struct sockaddr*)&name, (socklen_t)(offsetof(struct sockaddr_un, sun_path) + 1 + strlen(name.sun_path + 1))) == 0);
    assert(connect(raw, (struct sockaddr*)&target, sizeof target) == 0 && s->accept(server, &accepted, &peer) == 0 && bare(&peer));
    refuses(accepted, 1, MISSING);
    user = 7;
    assert(l_user(accepted, &user) == 0 && user == geteuid());
    assert(s->close(accepted) == 0 && close(raw) == 0);
    memset(longest, 'f', sizeof name.sun_path); longest[sizeof name.sun_path] = 0;
    memcpy(name.sun_path, longest, sizeof name.sun_path);
    raw = socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0);
    assert(raw >= 0 && bind(raw, (struct sockaddr*)&name, sizeof name) == 0 && connect(raw, (struct sockaddr*)&target, sizeof target) == 0);
    assert(s->accept(server, &accepted, &peer) == 0);
    answers(accepted, 1, longest);
    assert(s->close(accepted) == 0 && close(raw) == 0 && unlink(longest) == 0);

    /* Closing removes nothing: the paths stay, nobody listens on them any more, and the test removes them. */
    closed(server, server_fd);
    assert(is_socket(path) && is_socket(client_path));
    void *late = open_socket(LOCAL, TCP);
    assert(l_connect(late, path) == REFUSED && s->close(late) == 0);
    assert(unlink(path) == 0 && unlink(client_path) == 0 && unlink(in("relative")) == 0);
}

/* Datagram sockets: one bound, one connected to it, then a connected pair. */
static void datagram(void) {
    int a_fd, b_fd; dotnet_pal_socket_address from, address; uint8_t buffer[16]; size_t done = 7;
    const char *a_path = in("datagram-a"), *b_path = in("datagram-b");
    void *a = watched(UDP, &a_fd), *b = watched(UDP, &b_fd);
    assert(l_bind(a, a_path) == 0 && l_connect(b, a_path) == 0);
    answers(a, 0, a_path); answers(b, 1, a_path); refuses(b, 0, MISSING); refuses(a, 1, DOTNET_PAL_NOT_CONNECTED);
    assert(s->local_address(b, &address) == 0 && bare(&address) && s->peer_address(b, &address) == 0 && bare(&address));
    memset(&address, 0xff, sizeof address);
    assert(s->peer_address(a, &address) == DOTNET_PAL_NOT_CONNECTED && address.family == 0);
    /* The kernel names no sender for a socket without a path; the sender is a local socket all the same. */
    assert(s->send(b, TEXT("first"), 5, NULL, &done) == 0 && done == 5 && bits(a, READ, 5000 * MS) == READ && option(a, DOTNET_PAL_SOCKET_AVAILABLE) == 5);
    memset(&from, 0xff, sizeof from);
    assert(s->receive(a, buffer, sizeof buffer, 0, &from, &done) == 0 && done == 5 && memcmp(buffer, "first", 5) == 0 && bare(&from));
    /* A socket may take its name after it has connected; then the pair is connected both ways. */
    assert(l_bind(b, b_path) == 0 && l_connect(a, b_path) == 0);
    assert(strcmp(kernel_name(a_fd, 1), b_path) == 0 && strcmp(kernel_name(b_fd, 0), b_path) == 0);
    answers(b, 0, b_path); answers(a, 1, b_path);
    assert(s->send(a, TEXT("to b"), 4, NULL, &done) == 0 && done == 4 && s->send(b, NULL, 0, NULL, &done) == 0 && done == 0);
    memset(&from, 0xff, sizeof from);
    assert(s->receive(b, buffer, sizeof buffer, 0, &from, &done) == 0 && done == 4 && memcmp(buffer, "to b", 4) == 0 && bare(&from));
    memset(&from, 0xff, sizeof from); done = 7;
    assert(s->receive(a, buffer, sizeof buffer, 0, &from, &done) == 0 && done == 0 && bare(&from)); /* an empty datagram is a real message */
    assert(s->set_blocking(a, 0) == 0 && s->receive(a, buffer, sizeof buffer, 0, &from, &done) == DOTNET_PAL_WOULD_BLOCK && from.family == 0);
    /* Credentials belong to a connection; the kernel answers a connected datagram socket with user -1. */
    assert(kernel_user(a_fd) == UINT32_MAX);
    no_user(a, DOTNET_PAL_UNSUPPORTED);
    unsupported(a, DOTNET_PAL_SOCKET_MULTICAST_INTERFACE); unsupported(a, DOTNET_PAL_SOCKET_HOPS);
    /* A stream socket cannot connect to a datagram socket's path. */
    void *other = open_socket(LOCAL, TCP);
    assert(l_connect(other, a_path) == REFUSED && s->close(other) == 0);
    closed(a, a_fd); closed(b, b_fd);
    assert(unlink(a_path) == 0 && unlink(b_path) == 0);
}

/* Readiness and the connects that cannot wait. Linux finishes a local connect at once or not at all: with the backlog full it is
 * WOULD_BLOCK for a non-blocking socket and, for a blocking one with a SEND_TIMEOUT, TIMEOUT when that has passed. */
static size_t nonblocking(void) {
    int server_fd; const char *path = in("backlog"); void *clients[16], *accepted = (void*)1; dotnet_pal_socket_address peer; size_t made = 0, done = 7; uint32_t status = 0; uint8_t byte;
    void *server = listening(path, 1, &server_fd);
    assert(s->set_blocking(server, 0) == 0);
    memset(&peer, 0xff, sizeof peer);
    assert(s->accept(server, &accepted, &peer) == DOTNET_PAL_WOULD_BLOCK && accepted == NULL && peer.family == 0 && bits(server, READ, 0) == 0);
    while (made < 16 && status == 0) {
        clients[made] = open_socket(LOCAL, TCP);
        assert(s->set_blocking(clients[made], 0) == 0);
        status = l_connect(clients[made++], path);
        assert(status == 0 || status == DOTNET_PAL_IN_PROGRESS || status == DOTNET_PAL_WOULD_BLOCK);
        if (status == DOTNET_PAL_IN_PROGRESS) status = 0;
    }
    assert(status == DOTNET_PAL_WOULD_BLOCK && made >= 2);
    /* A connection the listener has not accepted yet is complete for its client: writable, no pending error, nothing to read. */
    assert(bits(clients[0], WRITE, 5000 * MS) == WRITE && option(clients[0], DOTNET_PAL_SOCKET_ERROR) == DOTNET_PAL_OK && bits(clients[0], READ, 0) == 0);
    assert(s->receive(clients[0], &byte, 1, 0, NULL, &done) == DOTNET_PAL_WOULD_BLOCK && done == 0);
    assert(bits(server, READ, 5000 * MS) == READ);
    void *patient = open_socket(LOCAL, TCP);
    assert(s->set_option(patient, DOTNET_PAL_SOCKET_SEND_TIMEOUT, 200) == 0);
    uint64_t start = now();
    assert(l_connect(patient, path) == DOTNET_PAL_TIMEOUT && now() - start >= 150 * MS);
    /* The accepted socket starts blocking whatever the listener is: with a receive timeout it waits, then reports the expiry. */
    for (size_t i = 0; i + 1 < made; ++i) {
        assert(s->accept(server, &accepted, &peer) == 0 && accepted && peer.family == LOCAL);
        if (i == 0) {
            assert(s->set_option(accepted, DOTNET_PAL_SOCKET_RECEIVE_TIMEOUT, 100) == 0);
            start = now();
            assert(s->receive(accepted, &byte, 1, 0, NULL, &done) == DOTNET_PAL_TIMEOUT && now() - start >= 80 * MS);
            assert(s->send(accepted, TEXT("r"), 1, NULL, &done) == 0 && bits(clients[0], READ, 5000 * MS) == READ && s->receive(clients[0], &byte, 1, 0, NULL, &done) == 0 && byte == 'r');
        }
        assert(s->close(accepted) == 0);
    }
    assert(s->accept(server, &accepted, &peer) == DOTNET_PAL_WOULD_BLOCK);
    /* Room again: the connect that could not wait succeeds now, as does the patient one. */
    assert(l_connect(clients[made - 1], path) == 0 && l_connect(patient, path) == 0);
    for (int i = 0; i < 2; ++i) { assert(bits(server, READ, 5000 * MS) == READ && s->accept(server, &accepted, &peer) == 0 && s->close(accepted) == 0); }
    /* The peer is gone: end of stream, reported to a reader as a hangup. */
    assert(bits(patient, READ, 5000 * MS) & HANGUP);
    assert(s->receive(patient, &byte, 1, 0, NULL, &done) == 0 && done == 0);
    for (size_t i = 0; i < made; ++i) assert(s->close(clients[i]) == 0);
    assert(s->close(patient) == 0);
    closed(server, server_fd);
    assert(unlink(path) == 0);
    return made - 1;
}

struct verdict { uint32_t bind_denied, bind_allowed, connect_denied, connect_allowed; };
static void try_as_this_user(struct verdict *v, int counted) {
    void *first = open_socket(LOCAL, TCP), *second = open_socket(LOCAL, TCP);
    const char *paths[4] = {in("locked/socket"), in("open/socket"), in("guarded"), in("welcome")};
    if (counted) {
        v->bind_denied = l_bind(first, paths[0]); v->bind_allowed = l_bind(first, paths[1]);
        v->connect_denied = l_connect(second, paths[2]); v->connect_allowed = l_connect(second, paths[3]);
    } else {
        v->bind_denied = l->bind(first, TEXT(paths[0]), strlen(paths[0])); v->bind_allowed = l->bind(first, TEXT(paths[1]), strlen(paths[1]));
        v->connect_denied = l->connect(second, TEXT(paths[2]), strlen(paths[2])); v->connect_allowed = l->connect(second, TEXT(paths[3]), strlen(paths[3]));
    }
    assert(s->close(first) == 0 && s->close(second) == 0);
}
/* The failures of bind and connect that the file system decides. */
static const char *failures(const char **read_only) {
    int fd, guarded_fd, welcome_fd; char longest[109], fits[108];
    void *socket = watched(TCP, &fd), *held = open_socket(LOCAL, TCP);
    assert(close(open(in("file"), O_CREAT | O_WRONLY | O_CLOEXEC, 0644)) == 0 && l_bind(held, in("held")) == 0);
    /* A path that exists, as a file or as another socket's name; then one the file system cannot make. */
    assert(l_bind(socket, in("file")) == DOTNET_PAL_ADDRESS_IN_USE && l_bind(socket, in("held")) == DOTNET_PAL_ADDRESS_IN_USE && l_bind(socket, root) == DOTNET_PAL_ADDRESS_IN_USE);
    assert(l_bind(socket, in("missing/socket")) == MISSING && l_bind(socket, in("file/socket")) == DOTNET_PAL_NOT_DIRECTORY);
    /* 107 bytes is the longest path; the scratch directory is the working directory, so a relative one can be that long. */
    memset(longest, 'p', 108); longest[108] = 0; memset(fits, 'p', 107); fits[107] = 0;
    assert(l_bind(socket, longest) == DOTNET_PAL_NAME_TOO_LONG && access(longest, F_OK) != 0 && l_connect(socket, longest) == DOTNET_PAL_NAME_TOO_LONG);
    /* None of the refused binds gave the socket a name. */
    assert(strcmp(kernel_name(fd, 0), "") == 0);
    refuses(socket, 0, MISSING);
    assert(l_bind(socket, fits) == 0 && strcmp(kernel_name(fd, 0), fits) == 0);
    answers(socket, 0, fits);
    closed(socket, fd);
    assert(unlink(fits) == 0);
    /* connect: nothing there, a file that is no socket, a socket nobody listens on, a path through a file. */
    socket = open_socket(LOCAL, TCP);
    assert(l_connect(socket, in("missing")) == MISSING && l_connect(socket, in("file")) == REFUSED && l_connect(socket, in("held")) == REFUSED);
    assert(l_connect(socket, in("file/socket")) == DOTNET_PAL_NOT_DIRECTORY && l_connect(socket, in("missing/socket")) == MISSING);
    assert(s->close(socket) == 0 && s->close(held) == 0 && unlink(in("held")) == 0);

    /* A read-only file system takes no new path. Needs the right to mount, which a container rarely has. */
    assert(mkdir(in("frozen"), 0755) == 0);
    if (mount("pal-local", in("frozen"), "tmpfs", MS_RDONLY, "size=1m") == 0) {
        socket = open_socket(LOCAL, TCP);
        assert(l_bind(socket, in("frozen/socket")) == DOTNET_PAL_READ_ONLY && s->close(socket) == 0 && umount(in("frozen")) == 0);
        *read_only = "checked";
    } else { assert(errno == EPERM || errno == EACCES); *read_only = "not-permitted"; }
    assert(rmdir(in("frozen")) == 0);

    /* Permissions: a directory that takes no new entry, and a listener whose path its user may not write. Root passes both, so
     * root asks as a user that exists nowhere else. */
    assert(mkdir(in("locked"), 0555) == 0 && mkdir(in("open"), 0777) == 0 && chmod(in("open"), 0777) == 0 && chmod(root, 0755) == 0);
    void *guarded = listening(in("guarded"), 4, &guarded_fd), *welcome = listening(in("welcome"), 4, &welcome_fd);
    assert(chmod(in("guarded"), 0) == 0 && chmod(in("welcome"), 0777) == 0);
    struct verdict v = {99, 99, 99, 99}; const char *as = "not-root";
    if (geteuid() != 0) try_as_this_user(&v, 1);
    else {
        int report[2], status = 0;
        assert(pipe(report) == 0);
        fflush(stdout);
        pid_t pid = fork();
        assert(pid >= 0);
        if (pid == 0) {
            uid_t nobody = 100000 + (uid_t)(getpid() % 100000);
            if (setgroups(0, NULL) != 0 || setgid(nobody) != 0 || setuid(nobody) != 0) _exit(2);
            try_as_this_user(&v, 0);
            _exit(write(report[1], &v, sizeof v) == (ssize_t)sizeof v ? 0 : 3);
        }
        close(report[1]);
        ssize_t got = read(report[0], &v, sizeof v);
        close(report[0]);
        assert(waitpid(pid, &status, 0) == pid && WIFEXITED(status));
        as = WEXITSTATUS(status) == 2 ? "no-other-user" : "another-user";
        assert(WEXITSTATUS(status) == 2 || (WEXITSTATUS(status) == 0 && got == (ssize_t)sizeof v));
    }
    if (strcmp(as, "no-other-user") != 0) assert(v.bind_denied == DOTNET_PAL_ACCESS_DENIED && v.bind_allowed == 0 && v.connect_denied == DOTNET_PAL_ACCESS_DENIED && v.connect_allowed == 0);
    closed(guarded, guarded_fd); closed(welcome, welcome_fd);
    assert(unlink(in("guarded")) == 0 && unlink(in("welcome")) == 0 && unlink(in("file")) == 0 && rmdir(in("locked")) == 0);
    if (strcmp(as, "no-other-user") != 0) assert(unlink(in("open/socket")) == 0);
    assert(rmdir(in("open")) == 0);
    return as;
}

/* A local socket has no IP address to give the sockets group, and an IP socket no path to give this one. */
static unsigned families(void) {
    dotnet_pal_socket_address loopback = {V4, 0, 0, {127, 0, 0, 1}}, at; size_t done = 7, needed = 7; uint32_t user = 7; unsigned refused = 0;
    dotnet_pal_sockets_stats before, after;
    void *stream = open_socket(LOCAL, TCP), *datagram = open_socket(LOCAL, UDP), *internet = open_socket(V4, UDP), *receiver = open_socket(V4, UDP);
    assert(s->bind(receiver, &loopback) == 0 && s->local_address(receiver, &at) == 0 && s->read_stats(&before, sizeof before) == 0);
    void *locals[2] = {stream, datagram};
    for (int i = 0; i < 2; ++i) {
        assert(s->bind(locals[i], &loopback) == INVALID && s->connect(locals[i], &at) == INVALID && s->send(locals[i], TEXT("x"), 1, &at, &done) == INVALID && done == 0);
        refused += 3;
        refuses(locals[i], 0, MISSING); /* nothing of that gave the socket a name */
    }
    assert(s->read_stats(&after, sizeof after) == 0 && after.rejected_or_failed == before.rejected_or_failed + 6 && after.bind_ok == before.bind_ok && after.connect_ok == before.connect_ok && after.send_ok == before.send_ok);
    assert(l_bind(internet, in("internet")) == INVALID && access(in("internet"), F_OK) != 0 && l_connect(internet, in("internet")) == INVALID);
    memset(out, 0xAA, sizeof out);
    assert(l_address(internet, 0, out, CAPACITY, &needed) == INVALID && needed == 0 && zero(out, CAPACITY) && l_address(internet, 1, out, CAPACITY, &needed) == INVALID);
    assert(l_user(internet, &user) == INVALID && user == UINT32_MAX);
    /* The IP socket still works as one. */
    assert(s->send(internet, TEXT("ip"), 2, &at, &done) == 0 && done == 2 && bits(receiver, READ, 5000 * MS) == READ);
    assert(s->close(stream) == 0 && s->close(datagram) == 0 && s->close(internet) == 0 && s->close(receiver) == 0);
    return refused;
}

/* Every argument the front end refuses before a provider runs: one count each, and the outputs it could reach are cleared. */
static unsigned validation(void) {
    dotnet_pal_local_sockets_stats before, after; size_t needed = 7; uint32_t user = 7; unsigned rejected = 0; uint8_t small[8];
    static uint8_t longest[DOTNET_PAL_MAX_NAME + 1];
    void *socket = open_socket(LOCAL, TCP);
    memset(longest, 'a', sizeof longest);
    assert(l->read_stats(&before, sizeof before) == 0);
    /* A path is 1 to MAX_NAME bytes without a NUL. */
    const struct { const uint8_t *path; size_t length; } paths[] = {{NULL, 1}, {TEXT("/s"), 0}, {longest, sizeof longest}, {TEXT("/a\0b"), 4}, {(const uint8_t*)(UINTPTR_MAX - 1), 8}};
    for (size_t i = 0; i < sizeof paths / sizeof *paths; ++i) { assert(l->bind(socket, paths[i].path, paths[i].length) == INVALID && l->connect(socket, paths[i].path, paths[i].length) == INVALID); rejected += 2; }
    assert(l->bind(NULL, TEXT("/s"), 2) == INVALID && l->connect(NULL, TEXT("/s"), 2) == INVALID);
    rejected += 2;
    /* The longest path the boundary takes reaches the provider, which has a shorter limit of its own. */
    assert(l->bind(socket, longest, DOTNET_PAL_MAX_NAME) == DOTNET_PAL_NAME_TOO_LONG && l->connect(socket, longest, DOTNET_PAL_MAX_NAME) == DOTNET_PAL_NAME_TOO_LONG);
    rejected += 2;
    /* address: a real place for the length, a buffer that is one, a socket, and peer is 0 or 1. */
    assert(l->address(socket, 0, small, sizeof small, NULL) == INVALID && l->address(socket, 0, small, sizeof small, (size_t*)((uintptr_t)&needed + 1)) == INVALID && needed == 7);
    assert(l->address(socket, 0, NULL, 1, &needed) == INVALID && l->address(socket, 0, small, (size_t)PTRDIFF_MAX + 1, &needed) == INVALID && l->address(socket, 0, (uint8_t*)(UINTPTR_MAX - 3), 8, &needed) == INVALID && needed == 7);
    memset(small, 0xAA, sizeof small);
    assert(l->address(NULL, 0, small, sizeof small, &needed) == INVALID && needed == 0 && small[0] == 0xAA);
    needed = 7;
    assert(l->address(socket, 2, small, sizeof small, &needed) == INVALID && needed == 0 && l->address(socket, UINT32_MAX, small, sizeof small, &needed) == INVALID);
    rejected += 8;
    /* peer_user: a real place for the user, which holds no user once the call has seen it. */
    assert(l->peer_user(socket, NULL) == INVALID && l->peer_user(socket, (uint32_t*)((uintptr_t)&user + 1)) == INVALID && user == 7);
    assert(l->peer_user(NULL, &user) == INVALID && user == UINT32_MAX);
    rejected += 3;
    assert(l->read_stats(NULL, sizeof after) == INVALID && l->read_stats(&after, sizeof after - 1) == INVALID && l->read_stats((void*)((uintptr_t)&after + 1), sizeof after) == INVALID);
    assert(l->read_stats(&after, sizeof after) == 0 && after.rejected_or_failed == before.rejected_or_failed + rejected);
    before.rejected_or_failed = after.rejected_or_failed;
    assert(memcmp(&before, &after, sizeof after) == 0);
    expected.rejected_or_failed += rejected;
    assert(s->close(socket) == 0);
    return rejected;
}

int main(int argc, char **argv) {
#ifdef PAL_HOST_TEST
    pal_local_sockets_fault = argc > 1 ? atoi(argv[1]) : 0;
#else
    (void)argc; (void)argv;
#endif
    signal(SIGPIPE, SIG_DFL);
    const dotnet_pal_api *api = dotnet_pal_get_api(2);
#ifdef PAL_HOST_TEST
    if (pal_local_sockets_fault == 1) { assert(!api); puts("LOCAL SOCKETS malformed host rejected"); return 0; }
#endif
    assert(api && api->header.struct_size >= DOTNET_PAL_LOCAL_SOCKETS_API_SIZE);
    assert((api->header.capabilities & DOTNET_PAL_CAP_LOCAL_SOCKETS) && (api->header.capabilities & DOTNET_PAL_CAP_SOCKETS));
    s = &api->sockets; l = &api->local_sockets;
    assert(l->bind && l->connect && l->address && l->peer_user && l->read_stats);
    dotnet_pal_local_sockets_stats stats;
    snprintf(root, sizeof root, "/tmp/pal-local-XXXXXX");
    assert(mkdtemp(root) && chdir(root) == 0);
#ifdef PAL_HOST_TEST
    if (pal_local_sockets_fault == 2) {
        /* The sockets are real; every answer about them is one the front end has to put right. */
        void *socket = open_socket(LOCAL, TCP); uint32_t user; size_t needed;
        for (int i = 0; i < 3; ++i) assert(l_bind(socket, in("s")) == DOTNET_PAL_OS_ERROR && l_connect(socket, in("s")) == DOTNET_PAL_OS_ERROR);
        /* No terminator, one inside, a text of length 1, no length, a status of the files group, one of bind, a need beyond any path, too small for a text that fits. */
        for (int i = 0; i < 8; ++i) refuses(socket, 0, DOTNET_PAL_OS_ERROR);
        /* A failure arrives without the outputs of the call that reported it. */
        refuses(socket, 1, DOTNET_PAL_NOT_CONNECTED); refuses(socket, 0, MISSING);
        memset(out, 0xAA, sizeof out); needed = 7;
        assert(l_address(socket, 0, out, 4, &needed) == MISSING && needed == 0 && zero(out, 4) && out[4] == 0xAA);
        /* A user written by a failing call is not the peer's: the output holds no user. */
        user = 7; assert(l_user(socket, &user) == DOTNET_PAL_NOT_CONNECTED && user == UINT32_MAX);
        user = 7; assert(l_user(socket, &user) == DOTNET_PAL_OS_ERROR && user == UINT32_MAX);
        user = 7; assert(l_user(socket, &user) == DOTNET_PAL_OS_ERROR && user == UINT32_MAX);
        assert(s->close(socket) == 0 && rmdir(root) == 0);
        assert(l->read_stats(&stats, sizeof stats) == 0 && memcmp(&stats, &expected, sizeof stats) == 0 && stats.rejected_or_failed == 20 && stats.bind_ok + stats.connect_ok + stats.address_ok + stats.peer_ok == 0);
        puts("LOCAL SOCKETS host errors sanitized"); return 0;
    }
#endif
    stream();
    datagram();
    size_t pending = nonblocking();
    const char *read_only = "", *as = failures(&read_only);
    unsigned other_family = families(), rejected = validation();
    assert(rmdir(root) == 0);
    /* Counters: one per answered call of each kind, one for everything refused, and nothing else. */
    assert(l->read_stats(&stats, sizeof stats) == 0 && memcmp(&stats, &expected, sizeof stats) == 0);
    assert(stats.bind_ok >= 9 && stats.connect_ok >= 8 && stats.address_ok >= 20 && stats.peer_ok == 4 && stats.rejected_or_failed >= rejected + 40);
    printf("LOCAL SOCKETS PASS user=%u binds=%llu connects=%llu addresses=%llu peers=%llu refused=%llu backlog_pending=%zu access_denied=%s read_only=%s ip_on_local=%u rejected=%u\n", (unsigned)geteuid(),
        (unsigned long long)stats.bind_ok, (unsigned long long)stats.connect_ok, (unsigned long long)stats.address_ok, (unsigned long long)stats.peer_ok,
        (unsigned long long)stats.rejected_or_failed, pending, as, read_only, other_family, rejected);
    return 0;
}

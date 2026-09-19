/* Conformance test of the sockets group on Linux: real TCP and UDP traffic over
 * loopback inside one process, readiness and wake, options, name resolution,
 * argument checks and counters. With PAL_HOST_TEST the same program runs
 * against the C host table; faults 1 and 2 check rejection and sanitizing. */
#define _GNU_SOURCE
#include "dotnet_pal.h"
#include <arpa/inet.h>
#include <assert.h>
#include <errno.h>
#include <fcntl.h>
#include <ifaddrs.h>
#include <net/if.h>
#include <netinet/in.h>
#include <netinet/tcp.h>
#include <poll.h>
#include <pthread.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <time.h>
#include <unistd.h>
#ifdef PAL_HOST_TEST
extern int pal_sockets_fault;
#endif
enum { V4 = DOTNET_PAL_FAMILY_IPV4, V6 = DOTNET_PAL_FAMILY_IPV6, TCP = DOTNET_PAL_SOCKET_STREAM, UDP = DOTNET_PAL_SOCKET_DATAGRAM,
    READ = DOTNET_PAL_POLL_READ, WRITE = DOTNET_PAL_POLL_WRITE, ERROR = DOTNET_PAL_POLL_ERROR, HANGUP = DOTNET_PAL_POLL_HANGUP };
#define MS UINT64_C(1000000)
#define TEXT(literal) ((const uint8_t*)(literal))
static const dotnet_pal_sockets_ops *s;
static unsigned rejected;
#define REJECT(call) do { assert((call) == DOTNET_PAL_INVALID_ARGUMENT); ++rejected; } while (0)
static uint64_t now(void) {
    struct timespec ts; assert(clock_gettime(CLOCK_MONOTONIC, &ts) == 0);
    return (uint64_t)ts.tv_sec * UINT64_C(1000000000) + (uint64_t)ts.tv_nsec;
}
static void pause_ms(long ms) { struct timespec ts = {ms / 1000, ms % 1000 * 1000000}; while (nanosleep(&ts, &ts) != 0) {} }
static int same(const dotnet_pal_socket_address *a, const dotnet_pal_socket_address *b) { return memcmp(a, b, sizeof *a) == 0; }
static dotnet_pal_socket_address loopback(uint32_t family, uint16_t port) {
    dotnet_pal_socket_address address = {(uint16_t)family, port, 0, {0}};
    if (family == V4) { address.address[0] = 127; address.address[3] = 1; } else address.address[15] = 1;
    return address;
}
static void *open_socket(uint32_t family, uint32_t kind) { void *socket = NULL; assert(s->create(family, kind, &socket) == 0 && socket); return socket; }
/* A socket bound to a loopback port the kernel picked; `at` receives the address. */
static void *bound(uint32_t family, uint32_t kind, dotnet_pal_socket_address *at) {
    void *socket = open_socket(family, kind);
    dotnet_pal_socket_address wanted = loopback(family, 0);
    assert(s->bind(socket, &wanted) == 0 && s->local_address(socket, at) == 0);
    wanted.port = at->port;
    assert(at->port != 0 && same(at, &wanted));
    return socket;
}
static void *listener(uint32_t family, dotnet_pal_socket_address *at) { void *socket = bound(family, TCP, at); assert(s->listen(socket, 8) == 0); return socket; }
/* A blocking connection through `server`: the peer the server sees is the client's own address. */
static void connected(void *server, const dotnet_pal_socket_address *at, void **client, void **accepted) {
    dotnet_pal_socket_address peer, address;
    *client = open_socket(at->family, TCP); *accepted = NULL;
    assert(s->connect(*client, at) == 0);
    assert(s->accept(server, accepted, &peer) == 0 && *accepted);
    assert(s->local_address(*client, &address) == 0 && same(&address, &peer));
    assert(s->peer_address(*accepted, &address) == 0 && same(&address, &peer));
    assert(s->peer_address(*client, &address) == 0 && same(&address, at));
    assert(s->local_address(*accepted, &address) == 0 && same(&address, at));
}
/* Sends `text` one way and expects exactly it on the blocking other side. */
static void transfer(void *from, void *to, const char *text) {
    size_t length = strlen(text), done = 7, total = 0; uint8_t buffer[64];
    assert(length <= sizeof buffer && s->send(from, TEXT(text), length, NULL, &done) == 0 && done == length);
    while (total < length) { assert(s->receive(to, buffer + total, sizeof buffer - total, 0, NULL, &done) == 0 && done > 0); total += done; }
    assert(memcmp(buffer, text, length) == 0);
}
static uint32_t bits(void *socket, uint32_t requested, uint64_t timeout_ns) {
    dotnet_pal_poll_entry entry = {socket, requested, 99}; size_t ready = 7;
    assert(s->poll(&entry, 1, timeout_ns, DOTNET_PAL_NO_CHANNEL, &ready) == 0 && ready == (entry.triggered != 0));
    return entry.triggered;
}
static uint64_t option(void *socket, uint32_t name) { uint64_t value = 7; assert(s->get_option(socket, name, &value) == 0); return value; }

static uint16_t tcp(void) {
    dotnet_pal_socket_address at, address, from; void *client, *accepted; uint8_t buffer[64]; size_t done = 7;
    void *server = listener(V4, &at);
    connected(server, &at, &client, &accepted);
    /* Both directions; a stream names no sender, so `from` stays the zero address. */
    assert(s->send(client, TEXT("ping"), 4, NULL, &done) == 0 && done == 4);
    memset(&from, 0xff, sizeof from);
    assert(s->receive(accepted, buffer, sizeof buffer, 0, &from, &done) == 0 && done == 4 && memcmp(buffer, "ping", 4) == 0 && from.family == 0 && from.port == 0);
    transfer(accepted, client, "pong!");
    /* Peeking leaves the bytes for the next receive, and AVAILABLE counts them. */
    assert(s->send(accepted, TEXT("again"), 5, NULL, &done) == 0 && done == 5);
    assert(s->receive(client, buffer, sizeof buffer, DOTNET_PAL_RECEIVE_PEEK, NULL, &done) == 0 && done == 5 && memcmp(buffer, "again", 5) == 0);
    assert(option(client, DOTNET_PAL_SOCKET_AVAILABLE) == 5);
    memset(buffer, 0, sizeof buffer);
    assert(s->receive(client, buffer, sizeof buffer, 0, NULL, &done) == 0 && done == 5 && memcmp(buffer, "again", 5) == 0);
    assert(option(client, DOTNET_PAL_SOCKET_AVAILABLE) == 0);
    /* An empty stream send has nothing to do; an empty buffer receives nothing. */
    assert(s->send(client, NULL, 0, NULL, &done) == 0 && done == 0);
    /* Half close: the peer reads end of stream, the other direction keeps working and the closed one is a broken pipe, not a signal. */
    assert(s->shutdown(client, DOTNET_PAL_SHUTDOWN_WRITE) == 0);
    assert(s->receive(accepted, buffer, sizeof buffer, 0, NULL, &done) == 0 && done == 0);
    assert(bits(accepted, READ, 0) == (READ | HANGUP) && bits(accepted, WRITE, 0) == WRITE);
    transfer(accepted, client, "late");
    assert(s->send(client, TEXT("x"), 1, NULL, &done) == DOTNET_PAL_BROKEN_PIPE && done == 0);
    assert(s->connect(client, &at) == DOTNET_PAL_ALREADY_CONNECTED);
    assert(s->close(client) == 0 && s->close(accepted) == 0);
    /* A socket with no connection, and the listener's port being taken. */
    void *idle = open_socket(V4, TCP);
    memset(&address, 0xff, sizeof address);
    assert(s->peer_address(idle, &address) == DOTNET_PAL_NOT_CONNECTED && address.family == 0);
    assert(s->receive(idle, buffer, sizeof buffer, 0, NULL, &done) == DOTNET_PAL_NOT_CONNECTED && done == 0);
    assert(s->bind(idle, &at) == DOTNET_PAL_ADDRESS_IN_USE);
    assert(s->close(idle) == 0 && s->close(server) == 0);
    /* Nobody listens on a port that was just closed. */
    void *refused = open_socket(V4, TCP);
    assert(s->connect(refused, &at) == DOTNET_PAL_CONNECTION_REFUSED);
    assert(s->close(refused) == 0);
    return at.port;
}
static uint32_t nonblocking(void) {
    dotnet_pal_socket_address at, peer, closed; void *accepted = (void*)1; uint8_t buffer[8]; size_t done = 7;
    void *server = listener(V4, &at), *client = open_socket(V4, TCP);
    assert(s->set_blocking(server, 0) == 0 && s->set_blocking(client, 0) == 0);
    memset(&peer, 0xff, sizeof peer);
    assert(s->accept(server, &accepted, &peer) == DOTNET_PAL_WOULD_BLOCK && accepted == NULL && peer.family == 0);
    uint32_t status = s->connect(client, &at);
    assert(status == DOTNET_PAL_IN_PROGRESS || status == 0);
    /* Completion shows as writability with no pending error. */
    assert(bits(client, WRITE, 5000 * MS) == WRITE && option(client, DOTNET_PAL_SOCKET_ERROR) == DOTNET_PAL_OK);
    assert(s->receive(client, buffer, sizeof buffer, 0, NULL, &done) == DOTNET_PAL_WOULD_BLOCK && done == 0);
    assert(bits(server, READ, 5000 * MS) == READ);
    assert(s->accept(server, &accepted, &peer) == 0 && accepted && peer.family == V4);
    /* The accepted socket starts blocking whatever the listener is: with a receive timeout it waits, then reports the expiry. */
    assert(s->set_option(accepted, DOTNET_PAL_SOCKET_RECEIVE_TIMEOUT, 200) == 0);
    uint64_t start = now();
    status = s->receive(accepted, buffer, sizeof buffer, 0, NULL, &done);
    assert(status == DOTNET_PAL_TIMEOUT && done == 0 && now() - start >= 150 * MS);
    uint32_t expiry = status;
    assert(s->set_option(accepted, DOTNET_PAL_SOCKET_RECEIVE_TIMEOUT, 0) == 0 && option(accepted, DOTNET_PAL_SOCKET_RECEIVE_TIMEOUT) == 0);
    assert(s->set_blocking(client, 1) == 0);
    transfer(accepted, client, "blocking again");
    assert(s->close(client) == 0 && s->close(accepted) == 0 && s->close(server) == 0);
    /* A failed non-blocking connect reports through poll and the ERROR option, which clears on read. */
    void *gone = listener(V4, &closed), *refused = open_socket(V4, TCP);
    assert(s->close(gone) == 0 && s->set_blocking(refused, 0) == 0);
    status = s->connect(refused, &closed);
    assert(status == DOTNET_PAL_IN_PROGRESS || status == DOTNET_PAL_CONNECTION_REFUSED);
    if (status == DOTNET_PAL_IN_PROGRESS) {
        assert(bits(refused, WRITE, 5000 * MS) & ERROR);
        assert(option(refused, DOTNET_PAL_SOCKET_ERROR) == DOTNET_PAL_CONNECTION_REFUSED && option(refused, DOTNET_PAL_SOCKET_ERROR) == DOTNET_PAL_OK);
    }
    assert(s->close(refused) == 0);
    return expiry;
}
static uint32_t woken = 99;
static void *waker(void *arg) { (void)arg; pause_ms(100); woken = s->wake(3); return NULL; }
static void polling(void) {
    dotnet_pal_socket_address at; void *client, *accepted; size_t ready = 7, done = 7; uint8_t byte;
    void *server = listener(V4, &at);
    connected(server, &at, &client, &accepted);
    /* Expiry: nothing is readable, so the whole time passes and nothing is triggered, with or without a channel. */
    dotnet_pal_poll_entry entry = {client, READ, 99};
    uint64_t start = now();
    assert(s->poll(&entry, 1, 100 * MS, DOTNET_PAL_NO_CHANNEL, &ready) == 0 && ready == 0 && entry.triggered == 0);
    uint64_t waited = now() - start;
    assert(waited >= 95 * MS && waited < 10000 * MS);
    start = now();
    assert(s->poll(&entry, 1, 50 * MS, 0, &ready) == 0 && ready == 0 && now() - start >= 45 * MS);
    /* An empty poll is a plain wait for its channel's wake. */
    start = now();
    assert(s->poll(NULL, 0, 50 * MS, DOTNET_PAL_POLL_CHANNELS - 1, &ready) == 0 && ready == 0 && now() - start >= 45 * MS);
    /* Level-triggered answers: only requested bits that are ready, at once with a zero timeout. */
    assert(bits(client, READ, 0) == 0 && bits(client, WRITE, 0) == WRITE && bits(client, 0, 0) == 0);
    assert(s->send(accepted, TEXT("x"), 1, NULL, &done) == 0 && done == 1);
    assert(bits(client, READ, DOTNET_PAL_INFINITE_NS) == READ && bits(client, READ, 0) == READ && bits(client, READ | WRITE, 0) == (READ | WRITE));
    /* More entries than a provider keeps on its stack; every other one has nothing to read. */
    static dotnet_pal_poll_entry many[300];
    for (size_t i = 0; i < 300; ++i) many[i] = (dotnet_pal_poll_entry){i % 2 ? client : accepted, READ, 99};
    assert(s->poll(many, 300, 0, 1, &ready) == 0 && ready == 150);
    for (size_t i = 0; i < 300; ++i) assert(many[i].triggered == (i % 2 ? READ : 0));
    assert(s->receive(client, &byte, 1, 0, NULL, &done) == 0 && done == 1 && byte == 'x');
    /* wake from another thread ends the endless poll on its channel with nothing triggered. */
    pthread_t thread;
    assert(pthread_create(&thread, NULL, waker, NULL) == 0);
    start = now(); entry.triggered = 99;
    assert(s->poll(&entry, 1, DOTNET_PAL_INFINITE_NS, 3, &ready) == 0 && ready == 0 && entry.triggered == 0 && now() - start >= 50 * MS);
    assert(pthread_join(thread, NULL) == 0 && woken == 0);
    /* A wake with no poll in progress waits for its own channel: polls on another channel or on none run to their timeout. */
    assert(s->wake(4) == 0);
    start = now();
    assert(s->poll(&entry, 1, 100 * MS, 5, &ready) == 0 && ready == 0 && now() - start >= 95 * MS);
    start = now();
    assert(s->poll(&entry, 1, 50 * MS, DOTNET_PAL_NO_CHANNEL, &ready) == 0 && ready == 0 && now() - start >= 45 * MS);
    /* It ends the next poll on channel 4, and only that one. */
    assert(s->poll(&entry, 1, DOTNET_PAL_INFINITE_NS, 4, &ready) == 0 && ready == 0 && entry.triggered == 0);
    start = now();
    assert(s->poll(&entry, 1, 50 * MS, 4, &ready) == 0 && ready == 0 && now() - start >= 45 * MS);
    assert(s->close(client) == 0 && s->close(accepted) == 0 && s->close(server) == 0);
}
static void round_trip(void *socket, uint32_t name, uint64_t value, uint64_t low, uint64_t high) {
    assert(s->set_option(socket, name, value) == 0);
    uint64_t got = option(socket, name);
    assert(got >= low && got <= high);
}
static void options(void) {
    void *stream = open_socket(V4, TCP), *datagram = open_socket(V4, UDP);
    static const uint32_t flags[] = {DOTNET_PAL_SOCKET_REUSE_ADDRESS, DOTNET_PAL_SOCKET_NO_DELAY, DOTNET_PAL_SOCKET_KEEP_ALIVE};
    for (size_t i = 0; i < sizeof flags / sizeof *flags; ++i) {
        assert(option(stream, flags[i]) == 0);
        round_trip(stream, flags[i], 1, 1, 1); round_trip(stream, flags[i], 0, 0, 0);
    }
    round_trip(datagram, DOTNET_PAL_SOCKET_BROADCAST, 1, 1, 1); round_trip(datagram, DOTNET_PAL_SOCKET_BROADCAST, 0, 0, 0);
    /* Sizes are the kernel's to round; they only have to stay real. */
    assert(option(stream, DOTNET_PAL_SOCKET_RECEIVE_BUFFER) != 0 && option(stream, DOTNET_PAL_SOCKET_SEND_BUFFER) != 0);
    round_trip(stream, DOTNET_PAL_SOCKET_RECEIVE_BUFFER, 65536, 1, UINT32_MAX); round_trip(stream, DOTNET_PAL_SOCKET_SEND_BUFFER, 65536, 1, UINT32_MAX);
    /* LINGER: 0 is off, n + 1 is on with n seconds, so 1 is "on, drop at once". */
    assert(option(stream, DOTNET_PAL_SOCKET_LINGER) == 0);
    round_trip(stream, DOTNET_PAL_SOCKET_LINGER, 1, 1, 1); round_trip(stream, DOTNET_PAL_SOCKET_LINGER, 6, 6, 6); round_trip(stream, DOTNET_PAL_SOCKET_LINGER, 0, 0, 0);
    /* Timeouts are milliseconds; the kernel may round up to its tick. */
    assert(option(stream, DOTNET_PAL_SOCKET_RECEIVE_TIMEOUT) == 0 && option(stream, DOTNET_PAL_SOCKET_SEND_TIMEOUT) == 0);
    round_trip(stream, DOTNET_PAL_SOCKET_RECEIVE_TIMEOUT, 1500, 1500, 1520); round_trip(stream, DOTNET_PAL_SOCKET_SEND_TIMEOUT, 2500, 2500, 2520);
    round_trip(stream, DOTNET_PAL_SOCKET_RECEIVE_TIMEOUT, 0, 0, 0); round_trip(stream, DOTNET_PAL_SOCKET_SEND_TIMEOUT, 0, 0, 0);
    assert(option(stream, DOTNET_PAL_SOCKET_ERROR) == DOTNET_PAL_OK && option(datagram, DOTNET_PAL_SOCKET_AVAILABLE) == 0);
    assert(s->close(stream) == 0 && s->close(datagram) == 0);
}
/* The descriptor the provider's next socket gets: the kernel hands out the lowest free one, and no other thread of this program is running. */
static int next_descriptor(void) { int fd = dup(0); assert(fd >= 0 && close(fd) == 0 && fcntl(fd, F_GETFD) == -1 && errno == EBADF); return fd; }
/* What the kernel itself holds for an option of that descriptor. */
static int own(int fd, int level, int name) { int value = -7; socklen_t length = sizeof value; assert(getsockopt(fd, level, name, &value, &length) == 0); return value; }
static void *watched(uint32_t family, uint32_t kind, int *fd) {
    *fd = next_descriptor();
    void *socket = open_socket(family, kind);
    assert(own(*fd, SOL_SOCKET, SO_TYPE) == (kind == TCP ? SOCK_STREAM : SOCK_DGRAM) && own(*fd, SOL_SOCKET, SO_DOMAIN) == (family == V4 ? AF_INET : AF_INET6));
    return socket;
}
static void closed(void *socket, int fd) { assert(s->close(socket) == 0 && fcntl(fd, F_GETFD) == -1 && errno == EBADF); }
static void unsupported(void *socket, uint32_t name) {
    uint64_t value = 7;
    assert(s->get_option(socket, name, &value) == DOTNET_PAL_UNSUPPORTED && value == 0 && s->set_option(socket, name, 1) == DOTNET_PAL_UNSUPPORTED);
}
/* The first interface that is up, carries multicast and has an IPv4 address; 0 when this machine has none. */
static unsigned multicast_interface(char *name) {
    struct ifaddrs *list = NULL; unsigned index = 0;
    assert(getifaddrs(&list) == 0);
    for (struct ifaddrs *i = list; i && !index; i = i->ifa_next) {
        unsigned wanted = IFF_UP | IFF_RUNNING | IFF_MULTICAST;
        if (!i->ifa_addr || i->ifa_addr->sa_family != AF_INET || (i->ifa_flags & wanted) != wanted || (i->ifa_flags & IFF_LOOPBACK)) continue;
        index = if_nametoindex(i->ifa_name); snprintf(name, IF_NAMESIZE, "%s", i->ifa_name);
    }
    freeifaddrs(list);
    return index;
}
/* One datagram on the test's own descriptor within `ms`: the hop limit it carried and the interface it came in on. 0 when none came. */
static int arrived(int fd, int ms, int *hops, unsigned *interface) {
    struct pollfd wait = {fd, POLLIN, 0}; char data[64]; union { char bytes[256]; struct cmsghdr align; } control;
    struct iovec part = {data, sizeof data}; struct msghdr message; struct in_pktinfo info;
    if (poll(&wait, 1, ms) <= 0) return 0;
    memset(&message, 0, sizeof message);
    message.msg_iov = &part; message.msg_iovlen = 1; message.msg_control = control.bytes; message.msg_controllen = sizeof control.bytes;
    assert(recvmsg(fd, &message, 0) >= 0);
    for (struct cmsghdr *c = CMSG_FIRSTHDR(&message); c; c = CMSG_NXTHDR(&message, c)) {
        if (c->cmsg_level == IPPROTO_IP && c->cmsg_type == IP_TTL) memcpy(hops, CMSG_DATA(c), sizeof *hops);
        if (c->cmsg_level == IPPROTO_IP && c->cmsg_type == IP_PKTINFO) { memcpy(&info, CMSG_DATA(c), sizeof info); *interface = (unsigned)info.ipi_ifindex; }
    }
    return 1;
}
/* Keep-alive tuning, hop limits and the multicast options: each is set, read back, and compared with the kernel's own answer for the
 * descriptor; the hop limits, the loopback flag and the interface also with the datagrams a socket of the test's own receives. */
static unsigned tuning(int six) {
    int fd, datagram_fd, one = 1, hops = -1; unsigned came_in = 0; char name[IF_NAMESIZE] = ""; size_t done = 7; uint64_t value = 7;
    void *stream = watched(V4, TCP, &fd), *datagram = watched(V4, UDP, &datagram_fd);
    /* The tuning starts at the kernel's defaults and does not switch keep-alive on. */
    assert(option(stream, DOTNET_PAL_SOCKET_KEEP_ALIVE_IDLE) == (uint64_t)own(fd, IPPROTO_TCP, TCP_KEEPIDLE));
    assert(option(stream, DOTNET_PAL_SOCKET_KEEP_ALIVE_INTERVAL) == (uint64_t)own(fd, IPPROTO_TCP, TCP_KEEPINTVL));
    assert(option(stream, DOTNET_PAL_SOCKET_KEEP_ALIVE_COUNT) == (uint64_t)own(fd, IPPROTO_TCP, TCP_KEEPCNT));
    round_trip(stream, DOTNET_PAL_SOCKET_KEEP_ALIVE_IDLE, 123, 123, 123); round_trip(stream, DOTNET_PAL_SOCKET_KEEP_ALIVE_INTERVAL, 45, 45, 45);
    round_trip(stream, DOTNET_PAL_SOCKET_KEEP_ALIVE_COUNT, 6, 6, 6);
    assert(own(fd, IPPROTO_TCP, TCP_KEEPIDLE) == 123 && own(fd, IPPROTO_TCP, TCP_KEEPINTVL) == 45 && own(fd, IPPROTO_TCP, TCP_KEEPCNT) == 6);
    assert(option(stream, DOTNET_PAL_SOCKET_KEEP_ALIVE) == 0 && own(fd, SOL_SOCKET, SO_KEEPALIVE) == 0);
    /* The kernel takes 1..32767 seconds and 1..127 probes; what it refuses is a bad argument and changes nothing. */
    assert(s->set_option(stream, DOTNET_PAL_SOCKET_KEEP_ALIVE_IDLE, 0) == DOTNET_PAL_INVALID_ARGUMENT && s->set_option(stream, DOTNET_PAL_SOCKET_KEEP_ALIVE_COUNT, 128) == DOTNET_PAL_INVALID_ARGUMENT);
    assert(option(stream, DOTNET_PAL_SOCKET_KEEP_ALIVE_IDLE) == 123 && option(stream, DOTNET_PAL_SOCKET_KEEP_ALIVE_COUNT) == 6);
    /* An option the protocol does not have: keep-alive tuning on datagrams, multicast on streams. */
    for (uint32_t o = DOTNET_PAL_SOCKET_KEEP_ALIVE_IDLE; o <= DOTNET_PAL_SOCKET_KEEP_ALIVE_COUNT; ++o) unsupported(datagram, o);
    for (uint32_t o = DOTNET_PAL_SOCKET_MULTICAST_HOPS; o <= DOTNET_PAL_SOCKET_MULTICAST_INTERFACE; ++o) unsupported(stream, o);
    /* Hop limits of both kinds of socket, and the multicast defaults: one hop, looped back, the kernel's choice of interface. */
    assert(option(stream, DOTNET_PAL_SOCKET_HOPS) == (uint64_t)own(fd, IPPROTO_IP, IP_TTL) && option(datagram, DOTNET_PAL_SOCKET_HOPS) == (uint64_t)own(datagram_fd, IPPROTO_IP, IP_TTL));
    round_trip(stream, DOTNET_PAL_SOCKET_HOPS, 33, 33, 33); round_trip(datagram, DOTNET_PAL_SOCKET_HOPS, 44, 44, 44); round_trip(datagram, DOTNET_PAL_SOCKET_HOPS, 255, 255, 255);
    assert(own(fd, IPPROTO_IP, IP_TTL) == 33 && own(datagram_fd, IPPROTO_IP, IP_TTL) == 255);
    assert(option(datagram, DOTNET_PAL_SOCKET_MULTICAST_HOPS) == 1 && option(datagram, DOTNET_PAL_SOCKET_MULTICAST_LOOPBACK) == 1 && option(datagram, DOTNET_PAL_SOCKET_MULTICAST_INTERFACE) == 0);
    round_trip(datagram, DOTNET_PAL_SOCKET_MULTICAST_HOPS, 0, 0, 0); round_trip(datagram, DOTNET_PAL_SOCKET_MULTICAST_HOPS, 255, 255, 255); round_trip(datagram, DOTNET_PAL_SOCKET_MULTICAST_HOPS, 5, 5, 5);
    round_trip(datagram, DOTNET_PAL_SOCKET_MULTICAST_LOOPBACK, 0, 0, 0);
    assert(own(datagram_fd, IPPROTO_IP, IP_MULTICAST_TTL) == 5 && own(datagram_fd, IPPROTO_IP, IP_MULTICAST_LOOP) == 0);
    round_trip(datagram, DOTNET_PAL_SOCKET_MULTICAST_LOOPBACK, 1, 1, 1);
    assert(own(datagram_fd, IPPROTO_IP, IP_MULTICAST_LOOP) == 1);
    /* An interface that does not exist is refused and the setting stays. */
    assert(s->set_option(datagram, DOTNET_PAL_SOCKET_MULTICAST_INTERFACE, 999999) == DOTNET_PAL_ADDRESS_NOT_AVAILABLE && option(datagram, DOTNET_PAL_SOCKET_MULTICAST_INTERFACE) == 0);
    /* Behaviour, seen by a socket that is the test's own: a unicast datagram carries HOPS. */
    int receiver = socket(AF_INET, SOCK_DGRAM | SOCK_CLOEXEC, 0);
    struct sockaddr_in any; socklen_t length = sizeof any;
    memset(&any, 0, sizeof any); any.sin_family = AF_INET;
    assert(receiver >= 0 && bind(receiver, (struct sockaddr*)&any, sizeof any) == 0 && getsockname(receiver, (struct sockaddr*)&any, &length) == 0);
    assert(setsockopt(receiver, IPPROTO_IP, IP_RECVTTL, &one, sizeof one) == 0 && setsockopt(receiver, IPPROTO_IP, IP_PKTINFO, &one, sizeof one) == 0);
    dotnet_pal_socket_address to = loopback(V4, ntohs(any.sin_port));
    assert(s->send(datagram, TEXT("hops"), 4, &to, &done) == 0 && done == 4 && arrived(receiver, 5000, &hops, &came_in) && hops == 255);
    round_trip(datagram, DOTNET_PAL_SOCKET_HOPS, 44, 44, 44);
    assert(s->send(datagram, TEXT("hops"), 4, &to, &done) == 0 && arrived(receiver, 5000, &hops, &came_in) && hops == 44);
    /* A multicast datagram leaves through MULTICAST_INTERFACE with MULTICAST_HOPS, and comes back only while MULTICAST_LOOPBACK is on. */
    unsigned index = multicast_interface(name);
    if (index) {
        struct ip_mreqn member; memset(&member, 0, sizeof member);
        assert(inet_pton(AF_INET, "239.255.77.78", &member.imr_multiaddr) == 1);
        member.imr_ifindex = (int)index;
        assert(setsockopt(receiver, IPPROTO_IP, IP_ADD_MEMBERSHIP, &member, sizeof member) == 0);
        dotnet_pal_socket_address group = {V4, ntohs(any.sin_port), 0, {239, 255, 77, 78}};
        round_trip(datagram, DOTNET_PAL_SOCKET_MULTICAST_INTERFACE, index, index, index);
        hops = -1; came_in = 0;
        assert(s->send(datagram, TEXT("group"), 5, &group, &done) == 0 && done == 5 && arrived(receiver, 5000, &hops, &came_in) && hops == 5 && came_in == index);
        round_trip(datagram, DOTNET_PAL_SOCKET_MULTICAST_LOOPBACK, 0, 0, 0);
        assert(s->send(datagram, TEXT("group"), 5, &group, &done) == 0 && done == 5 && !arrived(receiver, 300, &hops, &came_in));
        round_trip(datagram, DOTNET_PAL_SOCKET_MULTICAST_LOOPBACK, 1, 1, 1);
        assert(s->send(datagram, TEXT("group"), 5, &group, &done) == 0 && arrived(receiver, 5000, &hops, &came_in) && came_in == index);
        /* Through loopback instead, the same datagram does not reach a member on that interface: the option steers the traffic. */
        unsigned loopback_index = if_nametoindex("lo");
        round_trip(datagram, DOTNET_PAL_SOCKET_MULTICAST_INTERFACE, loopback_index, loopback_index, loopback_index);
        (void)s->send(datagram, TEXT("aside"), 5, &group, &done);
        assert(loopback_index != 0 && loopback_index != index && !arrived(receiver, 300, &hops, &came_in));
        round_trip(datagram, DOTNET_PAL_SOCKET_MULTICAST_INTERFACE, 0, 0, 0);
    } else puts("SOCKETS note: no interface that is up, carries multicast and has an IPv4 address; the multicast traffic checks are skipped");
    assert(close(receiver) == 0);
    closed(stream, fd); closed(datagram, datagram_fd);
    if (!six) return index;
    /* The same options at the IPv6 level. */
    stream = watched(V6, TCP, &fd); datagram = watched(V6, UDP, &datagram_fd);
    assert(option(stream, DOTNET_PAL_SOCKET_HOPS) == (uint64_t)own(fd, IPPROTO_IPV6, IPV6_UNICAST_HOPS));
    round_trip(stream, DOTNET_PAL_SOCKET_HOPS, 33, 33, 33); round_trip(datagram, DOTNET_PAL_SOCKET_HOPS, 1, 1, 1); round_trip(datagram, DOTNET_PAL_SOCKET_HOPS, 44, 44, 44);
    assert(own(fd, IPPROTO_IPV6, IPV6_UNICAST_HOPS) == 33 && own(datagram_fd, IPPROTO_IPV6, IPV6_UNICAST_HOPS) == 44 && own(datagram_fd, IPPROTO_IP, IP_TTL) != 44);
    assert(option(datagram, DOTNET_PAL_SOCKET_MULTICAST_HOPS) == 1 && option(datagram, DOTNET_PAL_SOCKET_MULTICAST_LOOPBACK) == 1 && option(datagram, DOTNET_PAL_SOCKET_MULTICAST_INTERFACE) == 0);
    round_trip(datagram, DOTNET_PAL_SOCKET_MULTICAST_HOPS, 7, 7, 7); round_trip(datagram, DOTNET_PAL_SOCKET_MULTICAST_LOOPBACK, 0, 0, 0);
    assert(own(datagram_fd, IPPROTO_IPV6, IPV6_MULTICAST_HOPS) == 7 && own(datagram_fd, IPPROTO_IPV6, IPV6_MULTICAST_LOOP) == 0);
    /* Loopback always has an index, whatever else this machine has. */
    unsigned lo = if_nametoindex("lo");
    assert(lo != 0);
    round_trip(datagram, DOTNET_PAL_SOCKET_MULTICAST_INTERFACE, lo, lo, lo);
    assert(own(datagram_fd, IPPROTO_IPV6, IPV6_MULTICAST_IF) == (int)lo);
    assert(s->set_option(datagram, DOTNET_PAL_SOCKET_MULTICAST_INTERFACE, 999999) == DOTNET_PAL_ADDRESS_NOT_AVAILABLE && option(datagram, DOTNET_PAL_SOCKET_MULTICAST_INTERFACE) == lo);
    for (uint32_t o = DOTNET_PAL_SOCKET_MULTICAST_HOPS; o <= DOTNET_PAL_SOCKET_MULTICAST_INTERFACE; ++o) unsupported(stream, o);
    for (uint32_t o = DOTNET_PAL_SOCKET_KEEP_ALIVE_IDLE; o <= DOTNET_PAL_SOCKET_KEEP_ALIVE_COUNT; ++o) unsupported(datagram, o);
    round_trip(stream, DOTNET_PAL_SOCKET_KEEP_ALIVE_IDLE, 77, 77, 77);
    assert(own(fd, IPPROTO_TCP, TCP_KEEPIDLE) == 77 && s->get_option(stream, DOTNET_PAL_SOCKET_KEEP_ALIVE_COUNT, &value) == 0 && value == (uint64_t)own(fd, IPPROTO_TCP, TCP_KEEPCNT));
    closed(stream, fd); closed(datagram, datagram_fd);
    return index;
}
static void udp(void) {
    dotnet_pal_socket_address a_at, b_at, from; uint8_t buffer[64]; size_t done = 7; static uint8_t oversized[70000];
    void *a = bound(V4, UDP, &a_at), *b = bound(V4, UDP, &b_at);
    assert(s->send(a, TEXT("datagram"), 8, &b_at, &done) == 0 && done == 8);
    assert(bits(b, READ, 5000 * MS) == READ && option(b, DOTNET_PAL_SOCKET_AVAILABLE) == 8);
    assert(s->receive(b, buffer, sizeof buffer, DOTNET_PAL_RECEIVE_PEEK, &from, &done) == 0 && done == 8 && same(&from, &a_at));
    memset(&from, 0xff, sizeof from);
    assert(s->receive(b, buffer, sizeof buffer, 0, &from, &done) == 0 && done == 8 && memcmp(buffer, "datagram", 8) == 0 && same(&from, &a_at));
    /* An empty datagram is a real message with a sender. */
    assert(s->send(b, NULL, 0, &a_at, &done) == 0 && done == 0);
    memset(&from, 0xff, sizeof from); done = 7;
    assert(s->receive(a, buffer, sizeof buffer, 0, &from, &done) == 0 && done == 0 && same(&from, &b_at));
    /* A connected datagram socket needs no destination. */
    assert(s->connect(a, &b_at) == 0 && s->send(a, TEXT("c"), 1, NULL, &done) == 0 && done == 1);
    assert(s->receive(b, buffer, sizeof buffer, 0, NULL, &done) == 0 && done == 1 && buffer[0] == 'c');
    assert(s->send(b, oversized, sizeof oversized, &a_at, &done) == DOTNET_PAL_MESSAGE_TOO_LARGE && done == 0);
    assert(s->set_blocking(b, 0) == 0 && s->receive(b, buffer, sizeof buffer, 0, &from, &done) == DOTNET_PAL_WOULD_BLOCK && from.family == 0);
    assert(s->close(a) == 0 && s->close(b) == 0);
}
static int ipv6(void) {
    void *probe = NULL, *client, *accepted; dotnet_pal_socket_address at = loopback(V6, 0), from; uint8_t buffer[8]; size_t done = 7;
    uint32_t status = s->create(V6, TCP, &probe);
    if (status == 0) { status = s->bind(probe, &at); assert(s->close(probe) == 0); }
    if (status == DOTNET_PAL_ADDRESS_NOT_AVAILABLE || status == DOTNET_PAL_UNSUPPORTED) { puts("SOCKETS note: no IPv6 loopback here, IPv6 checks skipped"); return 0; }
    assert(status == 0);
    void *server = open_socket(V6, TCP);
    round_trip(server, DOTNET_PAL_SOCKET_IPV6_ONLY, 1, 1, 1);
    assert(s->bind(server, &at) == 0 && s->listen(server, 8) == 0 && s->local_address(server, &at) == 0);
    assert(at.family == V6 && at.port != 0 && at.scope == 0 && at.address[15] == 1);
    connected(server, &at, &client, &accepted);
    transfer(client, accepted, "six"); transfer(accepted, client, "xis");
    assert(s->close(client) == 0 && s->close(accepted) == 0 && s->close(server) == 0);
    void *a = bound(V6, UDP, &at), *b = open_socket(V6, UDP);
    assert(s->send(b, TEXT("u6"), 2, &at, &done) == 0 && done == 2);
    assert(s->receive(a, buffer, sizeof buffer, 0, &from, &done) == 0 && done == 2 && from.family == V6 && from.port != 0 && from.address[15] == 1);
    assert(s->close(a) == 0 && s->close(b) == 0);
    return 1;
}
/* The peer is gone: sending ends as a status, never as SIGPIPE. */
/* RECEIVE_ERRORS: what ICMP reports about a datagram reaches the socket that sent it. The kernel tells a connected socket that nobody listens where it sent to
 * with or without the option; a socket that is not connected learns it only with the option, from its next receive or from the ERROR option. Either way the
 * failure is reported once and the socket is not in error afterwards. An IPv6 socket hears about what it sent to an IPv4-mapped address too. */
static unsigned network_errors(int six) {
    enum { ERRORS = DOTNET_PAL_SOCKET_RECEIVE_ERRORS };
    unsigned reported = 0; uint8_t byte; size_t done = 7;
    for (int pass = 0; pass < (six ? 3 : 1); ++pass) {
        uint32_t family = pass == 0 ? V4 : V6; int fd, level = pass == 0 ? IPPROTO_IP : IPPROTO_IPV6, name = pass == 0 ? IP_RECVERR : IPV6_RECVERR; dotnet_pal_socket_address nobody;
        /* A loopback port that was just given up: nobody listens there. */
        void *gone = bound(pass == 1 ? V6 : V4, UDP, &nobody), *sender = watched(family, UDP, &fd);
        assert(s->close(gone) == 0);
        if (pass == 2) {
            dotnet_pal_socket_address mapped = {V6, nobody.port, 0, {0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff, 127, 0, 0, 1}};
            nobody = mapped;
            assert(s->set_option(sender, DOTNET_PAL_SOCKET_IPV6_ONLY, 0) == 0);
        }
        assert(option(sender, ERRORS) == 0 && own(fd, level, name) == 0 && s->set_blocking(sender, 0) == 0);
        for (int on = 0; on < 3; ++on) {
            /* Off, on, off again: without the option the complaint is dropped and nothing ever comes. */
            assert(s->set_option(sender, ERRORS, on == 1) == 0 && option(sender, ERRORS) == (on == 1) && own(fd, level, name) == (on == 1));
            assert(s->send(sender, TEXT("x"), 1, &nobody, &done) == 0 && done == 1);
            if (on != 1) { assert(bits(sender, READ, 100 * MS) == 0 && s->receive(sender, &byte, 1, 0, NULL, &done) == DOTNET_PAL_WOULD_BLOCK); continue; }
            assert(bits(sender, READ, 5000 * MS) == ERROR);
            done = 7;
            assert(s->receive(sender, &byte, 1, 0, NULL, &done) == DOTNET_PAL_CONNECTION_REFUSED && done == 0);
            /* Reported once: the kernel keeps a copy in a queue the boundary never reads and would poll the socket as in error for it. */
            assert(bits(sender, READ, 0) == 0 && s->receive(sender, &byte, 1, 0, NULL, &done) == DOTNET_PAL_WOULD_BLOCK && option(sender, DOTNET_PAL_SOCKET_ERROR) == DOTNET_PAL_OK);
            assert(s->send(sender, TEXT("x"), 1, &nobody, &done) == 0 && bits(sender, READ, 5000 * MS) == ERROR);
            assert(option(sender, DOTNET_PAL_SOCKET_ERROR) == DOTNET_PAL_CONNECTION_REFUSED && option(sender, DOTNET_PAL_SOCKET_ERROR) == DOTNET_PAL_OK && bits(sender, READ, 0) == 0);
            ++reported;
        }
        /* Connected, which is how Ping uses the option: the same single report. */
        assert(s->set_option(sender, ERRORS, 1) == 0 && s->connect(sender, &nobody) == 0 && s->send(sender, TEXT("x"), 1, NULL, &done) == 0 && bits(sender, READ, 5000 * MS) == ERROR);
        assert(s->receive(sender, &byte, 1, 0, NULL, &done) == DOTNET_PAL_CONNECTION_REFUSED && bits(sender, READ, 0) == 0 && s->receive(sender, &byte, 1, 0, NULL, &done) == DOTNET_PAL_WOULD_BLOCK);
        closed(sender, fd);
    }
    /* A stream's failures are its connection's, and a local socket has no network to complain (the kernel would take IP_RECVERR on a stream). */
    void *others[3] = {open_socket(V4, TCP), open_socket(DOTNET_PAL_FAMILY_LOCAL, UDP), open_socket(DOTNET_PAL_FAMILY_LOCAL, TCP)};
    for (int i = 0; i < 3; ++i) { unsupported(others[i], ERRORS); assert(s->close(others[i]) == 0); }
    return reported;
}
static uint32_t peer_gone(void) {
    dotnet_pal_socket_address at; void *client, *accepted; size_t done = 7; uint32_t status = 0;
    void *server = listener(V4, &at);
    connected(server, &at, &client, &accepted);
    assert(s->close(accepted) == 0);
    for (int i = 0; i < 400 && status == 0; ++i) { status = s->send(client, TEXT("x"), 1, NULL, &done); pause_ms(5); }
    assert((status == DOTNET_PAL_BROKEN_PIPE || status == DOTNET_PAL_CONNECTION_RESET) && done == 0);
    assert(s->close(client) == 0 && s->close(server) == 0);
    return status;
}
static size_t names(void) {
    dotnet_pal_socket_address found[8]; size_t count = 7, loops = 0, needed = 7; uint8_t name[256]; char expected[256] = {0};
    assert(s->resolve(TEXT("localhost"), 9, 0, found, 8, &count) == 0 && count >= 1 && count <= 8);
    for (size_t i = 0; i < count; ++i) {
        dotnet_pal_socket_address v4 = loopback(V4, 0), v6 = loopback(V6, 0);
        v6.scope = found[i].scope;
        loops += same(&found[i], &v4) || same(&found[i], &v6);
    }
    assert(loops >= 1);
    size_t resolved = count;
    assert(s->resolve(TEXT("localhost"), 9, V4, found, 8, &count) == 0 && count >= 1);
    for (size_t i = 0; i < count; ++i) assert(found[i].family == V4);
    dotnet_pal_socket_address literal = loopback(V4, 0);
    assert(s->resolve(TEXT("127.0.0.1"), 9, 0, found, 1, &count) == 0 && count == 1 && same(&found[0], &literal));
    memset(found, 0xff, sizeof found);
    assert(s->resolve(TEXT("no-such-host.dotnet-pal.invalid"), 31, 0, found, 8, &count) == DOTNET_PAL_NOT_FOUND && count == 0 && found[0].family == 0);
    assert(gethostname(expected, sizeof expected - 1) == 0);
    assert(s->host_name(name, sizeof name, &needed) == 0 && needed == strlen(expected) + 1 && strcmp((char*)name, expected) == 0);
    memset(name, 'x', sizeof name); needed = 7;
    assert(s->host_name(name, 1, &needed) == DOTNET_PAL_BUFFER_TOO_SMALL && needed == strlen(expected) + 1 && name[0] == 0);
    assert(s->host_name(NULL, 0, &needed) == DOTNET_PAL_BUFFER_TOO_SMALL && needed == strlen(expected) + 1);
    return resolved;
}
static void validation(void) {
    dotnet_pal_sockets_stats before, after; dotnet_pal_socket_address at = loopback(V4, 0), bad = loopback(V4, 0), out[2];
    void *socket = open_socket(V4, UDP), *handle = (void*)1; uint8_t buffer[8], name[8]; size_t done = 7, count = 7; uint64_t value = 7;
    static dotnet_pal_poll_entry entries[2];
    bad.family = 9;
    assert(s->read_stats(&before, sizeof before) == 0);
    REJECT(s->create(0, TCP, &handle)); assert(handle == NULL);
    REJECT(s->create(4, TCP, &handle)); REJECT(s->create(V4, 0, &handle)); REJECT(s->create(V4, 4, &handle)); REJECT(s->create(V4, TCP, NULL));
    REJECT(s->create(DOTNET_PAL_FAMILY_LOCAL, DOTNET_PAL_SOCKET_RAW, &handle)); assert(handle == NULL); /* a local socket speaks no ICMP */
    REJECT(s->close(NULL));
    REJECT(s->bind(NULL, &at)); REJECT(s->bind(socket, NULL)); REJECT(s->bind(socket, &bad));
    REJECT(s->listen(NULL, 1));
    handle = (void*)1;
    REJECT(s->accept(NULL, &handle, &at)); assert(handle == NULL && at.family == 0);
    at = loopback(V4, 0);
    REJECT(s->accept(socket, NULL, &at)); REJECT(s->accept(socket, &handle, NULL));
    REJECT(s->connect(NULL, &at)); REJECT(s->connect(socket, NULL)); REJECT(s->connect(socket, &bad));
    REJECT(s->send(NULL, buffer, 1, NULL, &done)); REJECT(s->send(socket, NULL, 1, &at, &done)); REJECT(s->send(socket, buffer, 1, &bad, &done));
    REJECT(s->send(socket, buffer, 1, &at, NULL)); REJECT(s->send(socket, buffer, SIZE_MAX, &at, &done));
    REJECT(s->receive(NULL, buffer, 1, 0, NULL, &done)); REJECT(s->receive(socket, NULL, 1, 0, NULL, &done)); REJECT(s->receive(socket, buffer, 1, 2, NULL, &done));
    REJECT(s->receive(socket, buffer, 1, 0, NULL, NULL));
    REJECT(s->shutdown(NULL, DOTNET_PAL_SHUTDOWN_BOTH)); REJECT(s->shutdown(socket, 0)); REJECT(s->shutdown(socket, 4));
    REJECT(s->local_address(NULL, &out[0])); REJECT(s->local_address(socket, NULL)); REJECT(s->peer_address(NULL, &out[0])); REJECT(s->peer_address(socket, NULL));
    REJECT(s->set_blocking(NULL, 1)); REJECT(s->set_blocking(socket, 2));
    REJECT(s->get_option(NULL, DOTNET_PAL_SOCKET_ERROR, &value)); REJECT(s->get_option(socket, 0, &value)); REJECT(s->get_option(socket, 23, &value));
    REJECT(s->get_option(socket, DOTNET_PAL_SOCKET_ERROR, NULL));
    /* ERROR and AVAILABLE are read-only, flags are 0 or 1, and no value needs more than 32 bits. */
    REJECT(s->set_option(NULL, DOTNET_PAL_SOCKET_BROADCAST, 1)); REJECT(s->set_option(socket, 0, 1)); REJECT(s->set_option(socket, DOTNET_PAL_SOCKET_ERROR, 0));
    REJECT(s->set_option(socket, DOTNET_PAL_SOCKET_AVAILABLE, 0)); REJECT(s->set_option(socket, DOTNET_PAL_SOCKET_BROADCAST, 2));
    REJECT(s->set_option(socket, DOTNET_PAL_SOCKET_LINGER, UINT64_C(0x100000000))); REJECT(s->set_option(socket, 23, 1));
    REJECT(s->set_option(socket, DOTNET_PAL_SOCKET_PACKET_INFORMATION, 2)); REJECT(s->set_option(socket, DOTNET_PAL_SOCKET_DONT_FRAGMENT, 2));
    REJECT(s->set_option(socket, DOTNET_PAL_SOCKET_RECEIVE_ERRORS, 2));
    /* A hop limit is one byte and unicast traffic needs one hop, MULTICAST_LOOPBACK is a flag, and an interface index has 32 bits. */
    REJECT(s->set_option(socket, DOTNET_PAL_SOCKET_HOPS, 256)); REJECT(s->set_option(socket, DOTNET_PAL_SOCKET_MULTICAST_HOPS, 256));
    REJECT(s->set_option(socket, DOTNET_PAL_SOCKET_HOPS, 0)); REJECT(s->set_option(socket, DOTNET_PAL_SOCKET_KEEP_ALIVE_INTERVAL, 0));
    REJECT(s->set_option(socket, DOTNET_PAL_SOCKET_MULTICAST_LOOPBACK, 2)); REJECT(s->set_option(socket, DOTNET_PAL_SOCKET_MULTICAST_INTERFACE, UINT64_C(0x100000000)));
    /* poll: only READ and WRITE can be requested, every entry names a socket, and the array is bounded. */
    entries[0] = (dotnet_pal_poll_entry){socket, READ, 99}; entries[1] = (dotnet_pal_poll_entry){socket, ERROR, 99};
    REJECT(s->poll(entries, 2, 0, 0, &count)); assert(count == 0 && entries[0].triggered == 0 && entries[1].triggered == 0);
    entries[1] = (dotnet_pal_poll_entry){NULL, READ, 99};
    REJECT(s->poll(entries, 2, 0, 0, &count)); REJECT(s->poll(NULL, 1, 0, 0, &count)); REJECT(s->poll(entries, 1, 0, 0, NULL));
    REJECT(s->poll(entries, DOTNET_PAL_MAX_POLL + 1, 0, 0, &count));
    /* Channels end at DOTNET_PAL_POLL_CHANNELS; only a poll may go without one. */
    REJECT(s->poll(entries, 1, 0, DOTNET_PAL_POLL_CHANNELS, &count)); REJECT(s->wake(DOTNET_PAL_POLL_CHANNELS)); REJECT(s->wake(DOTNET_PAL_NO_CHANNEL));
    REJECT(s->resolve(NULL, 9, 0, out, 2, &count)); REJECT(s->resolve(TEXT("localhost"), 0, 0, out, 2, &count)); REJECT(s->resolve(TEXT("local\0host"), 10, 0, out, 2, &count));
    REJECT(s->resolve(TEXT("localhost"), 256, 0, out, 2, &count)); REJECT(s->resolve(TEXT("localhost"), 9, 3, out, 2, &count));
    REJECT(s->resolve(TEXT("localhost"), 9, 0, NULL, 2, &count)); REJECT(s->resolve(TEXT("localhost"), 9, 0, out, 0, &count));
    REJECT(s->resolve(TEXT("localhost"), 9, 0, out, 65, &count)); REJECT(s->resolve(TEXT("localhost"), 9, 0, out, 2, NULL));
    REJECT(s->host_name(name, sizeof name, NULL)); REJECT(s->host_name(NULL, 1, &count));
    /* Every rejection was counted as one, and none of them as a success. */
    assert(s->read_stats(&after, sizeof after) == 0);
    assert(after.rejected_or_failed - before.rejected_or_failed == rejected);
    before.rejected_or_failed = after.rejected_or_failed;
    assert(memcmp(&before, &after, sizeof after) == 0);
    assert(s->read_stats(NULL, sizeof after) == DOTNET_PAL_INVALID_ARGUMENT && s->read_stats(&after, sizeof after - 1) == DOTNET_PAL_INVALID_ARGUMENT);
    /* The refused values changed nothing. */
    assert(option(socket, DOTNET_PAL_SOCKET_HOPS) != 0 && option(socket, DOTNET_PAL_SOCKET_MULTICAST_HOPS) == 1 && option(socket, DOTNET_PAL_SOCKET_MULTICAST_LOOPBACK) == 1);
    assert(s->close(socket) == 0);
}
int main(int argc, char **argv) {
#ifdef PAL_HOST_TEST
    pal_sockets_fault = argc > 1 ? atoi(argv[1]) : 0;
#else
    (void)argc; (void)argv;
#endif
    /* A send to a closed peer must not depend on the process ignoring SIGPIPE. */
    signal(SIGPIPE, SIG_DFL);
    const dotnet_pal_api *api = dotnet_pal_get_api(2);
#ifdef PAL_HOST_TEST
    if (pal_sockets_fault == 1) { assert(!api); puts("SOCKETS malformed host rejected"); return 0; }
#endif
    assert(api && api->header.struct_size >= DOTNET_PAL_SOCKETS_API_SIZE);
    assert(api->header.capabilities & DOTNET_PAL_CAP_SOCKETS);
    s = &api->sockets;
    assert(s->create && s->close && s->bind && s->listen && s->accept && s->connect && s->send && s->receive && s->shutdown && s->local_address
        && s->peer_address && s->set_blocking && s->get_option && s->set_option && s->poll && s->wake && s->resolve && s->host_name && s->read_stats);
#ifdef PAL_HOST_TEST
    if (pal_sockets_fault == 2) {
        /* Honest calls build the sockets; the fault is on around each answer the front end has to sanitize. */
        dotnet_pal_socket_address at, peer, found[2]; void *client, *accepted = NULL, *broken = (void*)1; uint8_t buffer[8]; size_t done = 7, ready = 7, count = 7; uint64_t value = 7;
        pal_sockets_fault = 0;
        void *server = listener(V4, &at);
        client = open_socket(V4, TCP);
        assert(s->connect(client, &at) == 0);
        pal_sockets_fault = 2;
        assert(s->create(V4, TCP, &broken) == DOTNET_PAL_OS_ERROR && broken == NULL); /* success without a handle */
        memset(&peer, 0xff, sizeof peer);
        assert(s->accept(server, &accepted, &peer) == 0 && accepted && peer.family == 0 && peer.port == 0); /* unknown family: the zero address, the connection stays good */
        assert(s->send(client, TEXT("abcd"), 4, NULL, &done) == DOTNET_PAL_OS_ERROR && done == 0); /* more sent than offered */
        done = 7;
        assert(s->receive(accepted, buffer, sizeof buffer, 0, NULL, &done) == DOTNET_PAL_OS_ERROR && done == 0); /* more received than fits */
        memset(&peer, 0xff, sizeof peer);
        assert(s->local_address(client, &peer) == DOTNET_PAL_OS_ERROR && peer.family == 0 && peer.port == 0); /* success without an address */
        dotnet_pal_poll_entry entries[2] = {{client, READ, 0}, {accepted, 0, 0}};
        assert(s->poll(entries, 2, 0, 2, &ready) == 0 && ready == 2); /* recounted, and only defined bits that may be reported */
        assert(entries[0].triggered == (READ | ERROR | HANGUP) && entries[1].triggered == (ERROR | HANGUP));
        assert(s->get_option(client, DOTNET_PAL_SOCKET_ERROR, &value) == 0 && value == DOTNET_PAL_OS_ERROR); /* 99 is no status */
        value = 7;
        assert(s->get_option(client, DOTNET_PAL_SOCKET_ERROR, &value) == 0 && value == DOTNET_PAL_OS_ERROR); /* nor is a status with high bits set */
        memset(found, 0xff, sizeof found);
        assert(s->resolve(TEXT("localhost"), 9, 0, found, 2, &count) == DOTNET_PAL_OS_ERROR && count == 0 && found[0].family == 0 && found[1].family == 0);
        pal_sockets_fault = 0;
        transfer(client, accepted, "still good");
        assert(s->close(client) == 0 && s->close(accepted) == 0 && s->close(server) == 0);
        puts("SOCKETS host errors sanitized"); return 0;
    }
#endif
    uint16_t port = tcp();
    uint32_t expiry = nonblocking();
    polling();
    options();
    udp();
    int six = ipv6();
    unsigned multicast = tuning(six), complaints = network_errors(six);
    uint32_t gone = peer_gone();
    size_t resolved = names();
    validation();
    dotnet_pal_sockets_stats stats;
    assert(s->read_stats(&stats, sizeof stats) == 0);
    /* Every socket this program made was closed, and each kind of call succeeded at least as often as the fixed part of the run makes it. */
    assert(stats.close_ok == stats.create_ok + stats.accept_ok && stats.create_ok >= 20 && stats.accept_ok >= 4);
    assert(stats.bind_ok >= 7 && stats.listen_ok >= 5 && stats.connect_ok >= 5 && stats.send_ok >= 12 && stats.receive_ok >= 14 && stats.shutdown_ok == 1);
    assert(stats.address_ok >= 20 && stats.option_ok >= 76 && stats.poll_ok >= 17 && stats.wake_ok == 2 && stats.resolve_ok == 4);
    assert(stats.rejected_or_failed >= rejected + 10);
    assert(complaints == (six ? 3u : 1u));
    printf("SOCKETS PASS port=%u ipv6=%d receive_timeout=%s peer_gone=%s resolved=%zu sockets=%llu options=%llu multicast_interface=%u network_errors=%u rejected=%u\n", port, six,
        expiry == DOTNET_PAL_TIMEOUT ? "TIMEOUT" : "WOULD_BLOCK", gone == DOTNET_PAL_BROKEN_PIPE ? "BROKEN_PIPE" : "CONNECTION_RESET",
        resolved, (unsigned long long)(stats.create_ok + stats.accept_ok), (unsigned long long)stats.option_ok, multicast, complaints, rejected);
    return 0;
}

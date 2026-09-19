/* Conformance test of the packets group on Linux: datagrams sent inside one process
 * to a socket bound to every address, whose description (the interface, the address
 * it was sent to) is compared with what the kernel attaches to the same traffic on
 * a socket of the test's own; raw ICMP sockets of the sockets group that send an
 * echo request and read the reply, with the hop limit and the fragmentation switch
 * read back from the headers on the wire; argument checks and counters. With
 * PAL_HOST_TEST the same program runs against the C host tables; faults 1 and 2
 * check rejection and sanitizing. */
#define _GNU_SOURCE
#include "dotnet_pal.h"
#include <arpa/inet.h>
#include <assert.h>
#include <errno.h>
#include <fcntl.h>
#include <ifaddrs.h>
#include <net/if.h>
#include <netinet/in.h>
#include <poll.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/uio.h>
#include <time.h>
#include <unistd.h>
#ifdef PAL_HOST_TEST
extern int pal_packets_fault;
#endif
enum { V4 = DOTNET_PAL_FAMILY_IPV4, V6 = DOTNET_PAL_FAMILY_IPV6, LOCAL = DOTNET_PAL_FAMILY_LOCAL, TCP = DOTNET_PAL_SOCKET_STREAM, UDP = DOTNET_PAL_SOCKET_DATAGRAM,
    RAW = DOTNET_PAL_SOCKET_RAW, READ = DOTNET_PAL_POLL_READ, PEEK = DOTNET_PAL_RECEIVE_PEEK, INFORMATION = DOTNET_PAL_SOCKET_PACKET_INFORMATION,
    WHOLE = DOTNET_PAL_SOCKET_DONT_FRAGMENT, HOPS = DOTNET_PAL_SOCKET_HOPS, ERRORS = DOTNET_PAL_SOCKET_RECEIVE_ERRORS };
#define INVALID DOTNET_PAL_INVALID_ARGUMENT
#define MS UINT64_C(1000000)
#define TEXT(literal) ((const uint8_t*)(literal))
static const dotnet_pal_sockets_ops *s;
static const dotnet_pal_packets_ops *p;
/* What the counters have to say at the end: every call of the group is counted here. */
static dotnet_pal_packets_stats expected;
static const dotnet_pal_packet_info nowhere;
static const dotnet_pal_socket_address nobody;
static uint32_t loopback_index, other_index;
static dotnet_pal_socket_address other;
static char other_name[IF_NAMESIZE] = "none";

static uint64_t now(void) { struct timespec ts; assert(clock_gettime(CLOCK_MONOTONIC, &ts) == 0); return (uint64_t)ts.tv_sec * UINT64_C(1000000000) + (uint64_t)ts.tv_nsec; }
static uint32_t tally(uint32_t status) { ++*(status == DOTNET_PAL_OK ? &expected.receive_ok : &expected.rejected_or_failed); return status; }
/* One receive of the group and everything it wrote; what it did not write stays 0xAA. */
struct arrival { uint32_t status; size_t received; dotnet_pal_socket_address from; dotnet_pal_packet_info info; uint8_t data[128]; };
static struct arrival take(void *socket, size_t capacity, uint32_t flags) {
    struct arrival a;
    memset(&a, 0xAA, sizeof a); a.received = 7;
    assert(capacity <= sizeof a.data);
    a.status = tally(p->receive(socket, a.data, capacity, flags, &a.from, &a.info, sizeof a.info, &a.received));
    return a;
}
static int is(const struct arrival *a, const dotnet_pal_packet_info *info, const dotnet_pal_socket_address *from, const char *data, size_t size) {
    return a->status == DOTNET_PAL_OK && a->received == size && memcmp(a->data, data, size) == 0 && a->data[size] == 0xAA && memcmp(&a->info, info, sizeof *info) == 0 && memcmp(&a->from, from, sizeof *from) == 0;
}
/* A failure leaves no byte count, no sender and no description behind. */
static int failed(const struct arrival *a, uint32_t status) {
    return a->status == status && a->received == 0 && memcmp(&a->info, &nowhere, sizeof nowhere) == 0 && memcmp(&a->from, &nobody, sizeof nobody) == 0;
}
static dotnet_pal_socket_address loopback(uint32_t family, uint16_t port) {
    dotnet_pal_socket_address address = {(uint16_t)family, port, 0, {0}};
    if (family == V4) { address.address[0] = 127; address.address[3] = 1; } else address.address[15] = 1;
    return address;
}
/* The IPv4 address as an IPv6 socket that also takes IPv4 traffic names it. */
static dotnet_pal_socket_address mapped(dotnet_pal_socket_address a) {
    dotnet_pal_socket_address m = {V6, a.port, 0, {0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff, a.address[0], a.address[1], a.address[2], a.address[3]}};
    return m;
}
/* The descriptor the provider's next socket gets: the kernel hands out the lowest free one, and no other thread of this program is running. */
static int next_descriptor(void) { int fd = dup(0); assert(fd >= 0 && close(fd) == 0 && fcntl(fd, F_GETFD) == -1 && errno == EBADF); return fd; }
static int kernel(int fd, int level, int name) { int value = -7; socklen_t length = sizeof value; assert(getsockopt(fd, level, name, &value, &length) == 0); return value; }
static void *open_socket(uint32_t family, uint32_t kind) {
    void *socket = NULL;
    assert(s->create(family, kind, &socket) == 0 && socket && s->set_option(socket, DOTNET_PAL_SOCKET_RECEIVE_TIMEOUT, 5000) == 0);
    return socket;
}
static void closed(void *socket, int fd) { assert(s->close(socket) == 0 && fcntl(fd, F_GETFD) == -1 && errno == EBADF); }
static uint64_t option(void *socket, uint32_t name) { uint64_t value = 7; assert(s->get_option(socket, name, &value) == 0); return value; }
static void unsupported(void *socket, uint32_t name) {
    uint64_t value = 7;
    assert(s->get_option(socket, name, &value) == DOTNET_PAL_UNSUPPORTED && value == 0 && s->set_option(socket, name, 1) == DOTNET_PAL_UNSUPPORTED);
}
static uint32_t bits(void *socket, uint32_t requested, uint64_t timeout_ns) {
    dotnet_pal_poll_entry entry = {socket, requested, 99}; size_t ready = 7;
    assert(s->poll(&entry, 1, timeout_ns, DOTNET_PAL_NO_CHANNEL, &ready) == 0 && ready == (entry.triggered != 0));
    return entry.triggered;
}
static void sent(void *socket, const void *data, size_t size, const dotnet_pal_socket_address *to) {
    size_t done = 7;
    assert(s->send(socket, data, size, to, &done) == 0 && done == size);
}
static int waits(int fd) { struct pollfd slot = {fd, POLLIN, 0}; return poll(&slot, 1, 5000) == 1; }

/* The loopback interface, and the first other one that is up and has an IPv4 address. */
static void interfaces(void) {
    struct ifaddrs *list = NULL;
    loopback_index = if_nametoindex("lo");
    assert(loopback_index != 0 && getifaddrs(&list) == 0);
    for (struct ifaddrs *i = list; i && other_index == 0; i = i->ifa_next) {
        if (!i->ifa_addr || i->ifa_addr->sa_family != AF_INET || (i->ifa_flags & IFF_LOOPBACK) || !(i->ifa_flags & IFF_UP)) continue;
        other.family = V4; memcpy(other.address, &((struct sockaddr_in*)i->ifa_addr)->sin_addr, 4);
        other_index = if_nametoindex(i->ifa_name); snprintf(other_name, sizeof other_name, "%s", i->ifa_name);
    }
    freeifaddrs(list);
}

/* A socket of the group bound to every address of its family, and one of the test's own set up the same way. */
struct pair { void *receiver; int fd, twin; uint16_t port, twin_port; uint32_t family; };
static struct pair everywhere(uint32_t family, int dual) {
    struct pair w = {NULL, next_descriptor(), -1, 0, 0, family}; dotnet_pal_socket_address any = {(uint16_t)family, 0, 0, {0}}, at; int on = 1, only = !dual;
    w.receiver = open_socket(family, UDP);
    if (family == V6) assert(s->set_option(w.receiver, DOTNET_PAL_SOCKET_IPV6_ONLY, (uint64_t)only) == 0);
    assert(s->bind(w.receiver, &any) == 0 && s->local_address(w.receiver, &at) == 0 && at.port != 0);
    w.port = at.port;
    struct sockaddr_storage name; socklen_t length = sizeof name;
    memset(&name, 0, sizeof name); name.ss_family = family == V6 ? AF_INET6 : AF_INET;
    w.twin = socket(name.ss_family, SOCK_DGRAM | SOCK_CLOEXEC, 0);
    assert(w.twin >= 0 && (family == V4 || setsockopt(w.twin, IPPROTO_IPV6, IPV6_V6ONLY, &only, sizeof only) == 0));
    assert(bind(w.twin, (struct sockaddr*)&name, family == V6 ? sizeof(struct sockaddr_in6) : sizeof(struct sockaddr_in)) == 0 && getsockname(w.twin, (struct sockaddr*)&name, &length) == 0);
    w.twin_port = ntohs(((struct sockaddr_in*)&name)->sin_port); /* sin_port and sin6_port are the same two bytes */
    assert(setsockopt(w.twin, family == V6 ? IPPROTO_IPV6 : IPPROTO_IP, family == V6 ? IPV6_RECVPKTINFO : IP_PKTINFO, &on, sizeof on) == 0);
    return w;
}
/* What the kernel attached to the next datagram of the test's own socket, in the boundary's terms. */
static dotnet_pal_packet_info kernel_info(int fd) {
    dotnet_pal_packet_info info = nowhere; char data[64], control[256]; struct iovec part = {data, sizeof data}; struct msghdr message;
    memset(&message, 0, sizeof message);
    message.msg_iov = &part; message.msg_iovlen = 1; message.msg_control = control; message.msg_controllen = sizeof control;
    assert(waits(fd) && recvmsg(fd, &message, 0) >= 0);
    for (struct cmsghdr *record = CMSG_FIRSTHDR(&message); record; record = CMSG_NXTHDR(&message, record)) {
        if (record->cmsg_level == IPPROTO_IP && record->cmsg_type == IP_PKTINFO) {
            struct in_pktinfo arrived; memcpy(&arrived, CMSG_DATA(record), sizeof arrived);
            info.interface_index = (uint32_t)arrived.ipi_ifindex; info.destination.family = V4; memcpy(info.destination.address, &arrived.ipi_addr, 4);
        }
        if (record->cmsg_level == IPPROTO_IPV6 && record->cmsg_type == IPV6_PKTINFO) {
            struct in6_pktinfo arrived; memcpy(&arrived, CMSG_DATA(record), sizeof arrived);
            info.interface_index = arrived.ipi6_ifindex; info.destination.family = V6; memcpy(info.destination.address, &arrived.ipi6_addr, 16);
        }
    }
    return info;
}
/* One datagram to the group's socket and one to the test's own, from the same sender to the same address. The group reports the
 * address the datagram was sent to and the interface that has it, which is what the kernel attached for the test, and names the
 * sender as sockets.receive does. A receive that only peeks describes the datagram as the one that takes it. */
static void arrives(const struct pair *w, void *sender, dotnet_pal_socket_address to, uint32_t interface) {
    dotnet_pal_socket_address source, seen = w->family == V6 && to.family == V4 ? mapped(to) : to; dotnet_pal_packet_info wanted = {interface, 0, seen}, attached;
    to.port = w->port;
    sent(sender, "where?", 6, &to);
    assert(s->local_address(sender, &source) == 0 && source.port != 0 && bits(w->receiver, READ, 5000 * MS) == READ);
    seen.port = source.port; /* a datagram to one of this host's addresses leaves from that address */
    struct arrival peeked = take(w->receiver, 64, PEEK), taken = take(w->receiver, 64, 0);
    assert(is(&peeked, &wanted, &seen, "where?", 6) && is(&taken, &wanted, &seen, "where?", 6));
    to.port = w->twin_port;
    sent(sender, "where?", 6, &to);
    attached = kernel_info(w->twin);
    assert(memcmp(&attached, &wanted, sizeof wanted) == 0);
}
/* Everything about one family: `to` is the loopback address a sender of `sends` reaches the pair at. */
static void datagrams(uint32_t family, int dual, uint32_t sends) {
    struct pair w = everywhere(family, dual); void *sender = open_socket(sends, UDP); dotnet_pal_socket_address to = loopback(sends, w.port), source, seen;
    dotnet_pal_packet_info here = {loopback_index, 0, family == V6 && sends == V4 ? mapped(loopback(V4, 0)) : loopback(family, 0)};
    dotnet_pal_sockets_stats before, after; size_t done = 7; uint8_t small[4]; int level = family == V6 ? IPPROTO_IPV6 : IPPROTO_IP, name = family == V6 ? IPV6_RECVPKTINFO : IP_PKTINFO;
    /* Without the option a datagram has a sender and no description. */
    assert(option(w.receiver, INFORMATION) == 0 && kernel(w.fd, level, name) == 0);
    sent(sender, "plain", 5, &to);
    assert(s->local_address(sender, &source) == 0);
    seen = family == V6 && sends == V4 ? mapped(loopback(V4, source.port)) : loopback(family, source.port);
    struct arrival a = take(w.receiver, 64, 0);
    assert(is(&a, &nowhere, &seen, "plain", 5));
    assert(s->set_option(w.receiver, INFORMATION, 1) == 0 && option(w.receiver, INFORMATION) == 1 && kernel(w.fd, level, name) == 1);
    arrives(&w, sender, loopback(sends, 0), loopback_index);
    if (other_index != 0 && sends == V4) arrives(&w, sender, other, other_index);
    /* A datagram longer than the buffer is cut and the rest discarded, as in sockets.receive; its description is whole. No room at all takes it too. */
    sent(sender, "0123456789", 10, &to); sent(sender, "next", 4, &to); sent(sender, "gone", 4, &to);
    a = take(w.receiver, 4, 0);
    assert(is(&a, &here, &seen, "0123", 4));
    a = take(w.receiver, 64, PEEK);
    assert(is(&a, &here, &seen, "next", 4));
    a = take(w.receiver, 64, 0);
    assert(is(&a, &here, &seen, "next", 4));
    dotnet_pal_packet_info info; dotnet_pal_socket_address from;
    memset(&info, 0xAA, sizeof info); memset(&from, 0xAA, sizeof from);
    assert(tally(p->receive(w.receiver, NULL, 0, 0, &from, &info, sizeof info, &done)) == 0 && done == 0 && memcmp(&info, &here, sizeof here) == 0 && memcmp(&from, &seen, sizeof seen) == 0);
    /* The sender is optional, and a caller built against a longer description gets the part this version knows. The sockets group counts none of this. */
    struct { dotnet_pal_packet_info info; uint64_t later; } longer = {nowhere, UINT64_C(0xAAAAAAAAAAAAAAAA)};
    sent(sender, "alone", 5, &to);
    assert(s->read_stats(&before, sizeof before) == 0);
    assert(tally(p->receive(w.receiver, small, sizeof small, 0, NULL, &longer.info, sizeof longer, &done)) == 0 && done == 4 && memcmp(small, "alon", 4) == 0);
    assert(memcmp(&longer.info, &here, sizeof here) == 0 && longer.later == UINT64_C(0xAAAAAAAAAAAAAAAA));
    assert(s->read_stats(&after, sizeof after) == 0 && memcmp(&before, &after, sizeof after) == 0);
    /* Nothing to read: a timeout for a socket that waits, "not now" for one that does not, and no output either way. */
    assert(s->set_option(w.receiver, DOTNET_PAL_SOCKET_RECEIVE_TIMEOUT, 100) == 0);
    uint64_t start = now();
    a = take(w.receiver, 64, 0);
    assert(failed(&a, DOTNET_PAL_TIMEOUT) && now() - start >= 80 * MS && a.data[0] == 0xAA);
    assert(s->set_blocking(w.receiver, 0) == 0);
    a = take(w.receiver, 64, 0);
    assert(failed(&a, DOTNET_PAL_WOULD_BLOCK));
    a = take(w.receiver, 64, PEEK);
    assert(failed(&a, DOTNET_PAL_WOULD_BLOCK));
    a = take(w.receiver, 0, 0); /* no room is no reason to answer without a datagram */
    assert(failed(&a, DOTNET_PAL_WOULD_BLOCK));
    /* Off again: the next datagram has no description. */
    assert(s->set_option(w.receiver, INFORMATION, 0) == 0 && option(w.receiver, INFORMATION) == 0 && kernel(w.fd, level, name) == 0);
    sent(sender, "quiet", 5, &to);
    assert(bits(w.receiver, READ, 5000 * MS) == READ);
    a = take(w.receiver, 64, 0);
    assert(is(&a, &nowhere, &seen, "quiet", 5));
    closed(w.receiver, w.fd);
    assert(s->close(sender) == 0 && close(w.twin) == 0);
}

/* A failure of the transfer arrives as sockets.receive reports it: nobody listens where a connected socket sent to, which a socket that is not connected
 * learns with RECEIVE_ERRORS. The failure is reported once; the socket is not in error afterwards. */
static void refused(void) {
    dotnet_pal_socket_address at = loopback(V4, 0); size_t done = 7;
    void *gone = open_socket(V4, UDP), *client = open_socket(V4, UDP), *stranger = open_socket(V4, UDP);
    assert(s->bind(gone, &at) == 0 && s->local_address(gone, &at) == 0 && s->close(gone) == 0);
    assert(s->set_option(client, INFORMATION, 1) == 0 && s->connect(client, &at) == 0 && s->send(client, TEXT("anyone?"), 7, NULL, &done) == 0);
    assert(bits(client, READ, 5000 * MS) != 0);
    struct arrival a = take(client, 64, 0);
    assert(failed(&a, DOTNET_PAL_CONNECTION_REFUSED) && s->close(client) == 0);
    assert(s->set_option(stranger, INFORMATION, 1) == 0 && s->set_option(stranger, ERRORS, 1) == 0 && s->set_blocking(stranger, 0) == 0);
    sent(stranger, "anyone?", 7, &at);
    assert(bits(stranger, READ, 5000 * MS) == DOTNET_PAL_POLL_ERROR);
    a = take(stranger, 64, 0);
    assert(failed(&a, DOTNET_PAL_CONNECTION_REFUSED) && bits(stranger, READ, 0) == 0);
    a = take(stranger, 64, 0);
    assert(failed(&a, DOTNET_PAL_WOULD_BLOCK) && s->close(stranger) == 0);
}

/* Only a datagram has a destination of its own: a stream's is its local address, and a local socket has no IP address. */
static void other_sockets(void) {
    void *stream = open_socket(V4, TCP), *stream6 = NULL, *local = open_socket(LOCAL, UDP), *local_stream = open_socket(LOCAL, TCP);
    void *refusing[3] = {stream, local, local_stream};
    for (int i = 0; i < 3; ++i) { struct arrival a = take(refusing[i], 64, 0); assert(failed(&a, INVALID) && a.data[0] == 0xAA); unsupported(refusing[i], INFORMATION); unsupported(refusing[i], ERRORS); }
    unsupported(local, WHOLE); unsupported(local_stream, WHOLE);
    /* What a stream sends may be kept whole too. */
    assert(option(stream, WHOLE) == 0 && s->set_option(stream, WHOLE, 1) == 0 && option(stream, WHOLE) == 1);
    if (s->create(V6, TCP, &stream6) == 0) { unsupported(stream6, WHOLE); unsupported(stream6, INFORMATION); assert(s->close(stream6) == 0); }
    for (int i = 0; i < 3; ++i) assert(s->close(refusing[i]) == 0);
}

/* Every argument the front end refuses before a provider runs: one count each, and the outputs it could reach are cleared. */
static unsigned validation(void) {
    dotnet_pal_packets_stats before, after; dotnet_pal_packet_info info; dotnet_pal_socket_address from; size_t done = 7; uint8_t buffer[8]; unsigned rejected = 0;
    void *socket = open_socket(V4, UDP);
    assert(p->read_stats(&before, sizeof before) == 0);
    memset(&info, 0xAA, sizeof info); memset(&from, 0xAA, sizeof from);
    /* Places for the byte count and the description, which has at least this version's size, and a sender that is one when given: nothing is written before they are known. */
    assert(p->receive(socket, buffer, sizeof buffer, 0, &from, &info, sizeof info, NULL) == INVALID && p->receive(socket, buffer, sizeof buffer, 0, &from, &info, sizeof info, (size_t*)((uintptr_t)&done + 1)) == INVALID);
    assert(p->receive(socket, buffer, sizeof buffer, 0, &from, NULL, sizeof info, &done) == INVALID && p->receive(socket, buffer, sizeof buffer, 0, &from, (void*)((uintptr_t)&info + 1), sizeof info, &done) == INVALID);
    assert(p->receive(socket, buffer, sizeof buffer, 0, &from, &info, sizeof info - 1, &done) == INVALID && p->receive(socket, buffer, sizeof buffer, 0, &from, &info, 0, &done) == INVALID);
    assert(p->receive(socket, buffer, sizeof buffer, 0, (void*)((uintptr_t)&from + 1), &info, sizeof info, &done) == INVALID);
    assert(done == 7 && info.interface_index == 0xAAAAAAAAu && from.family == 0xAAAA);
    rejected += 7;
    /* A socket, a buffer that is one, and no flag but PEEK: the outputs hold nothing afterwards. */
    const struct { void *socket; uint8_t *data; size_t capacity; uint32_t flags; } bad[] = {{NULL, buffer, sizeof buffer, 0}, {socket, NULL, 1, 0}, {socket, buffer, (size_t)PTRDIFF_MAX + 1, 0},
        {socket, (uint8_t*)(UINTPTR_MAX - 3), 8, 0}, {socket, buffer, sizeof buffer, 2}, {socket, buffer, sizeof buffer, PEEK | 4}, {socket, buffer, sizeof buffer, UINT32_MAX}};
    for (size_t i = 0; i < sizeof bad / sizeof *bad; ++i) {
        memset(&info, 0xAA, sizeof info); memset(&from, 0xAA, sizeof from); done = 7;
        assert(p->receive(bad[i].socket, bad[i].data, bad[i].capacity, bad[i].flags, &from, &info, sizeof info, &done) == INVALID);
        assert(done == 0 && memcmp(&info, &nowhere, sizeof info) == 0 && memcmp(&from, &nobody, sizeof from) == 0);
        ++rejected;
    }
    assert(p->read_stats(NULL, sizeof after) == INVALID && p->read_stats(&after, sizeof after - 1) == INVALID && p->read_stats((void*)((uintptr_t)&after + 1), sizeof after) == INVALID);
    assert(p->read_stats(&after, sizeof after) == 0 && after.rejected_or_failed == before.rejected_or_failed + rejected && after.receive_ok == before.receive_ok);
    expected.rejected_or_failed += rejected;
    assert(s->close(socket) == 0);
    return rejected;
}

/* The Internet checksum of `size` bytes on top of `sum`; over a message that carries its own it comes out as 0. */
static uint16_t checksum(const uint8_t *bytes, size_t size, uint32_t sum) {
    for (size_t i = 0; i + 1 < size; i += 2) sum += (uint32_t)(bytes[i] << 8 | bytes[i + 1]);
    if (size & 1) sum += (uint32_t)bytes[size - 1] << 8;
    while (sum >> 16) sum = (sum & 0xffff) + (sum >> 16);
    return (uint16_t)~sum;
}
static const char payload[] = "pal-packets-echo";
enum { ECHO = 8 + sizeof payload - 1 };
static uint16_t echo_id;
/* An echo request as Ping builds it: type, code 0, checksum, identifier, sequence number, payload. ICMPv6's checksum covers the addresses, so the kernel fills it in. */
static void ask(void *raw, int v6, uint16_t sequence, const dotnet_pal_socket_address *to) {
    uint8_t request[ECHO] = {v6 ? 128 : 8, 0, 0, 0, (uint8_t)(echo_id >> 8), (uint8_t)echo_id, (uint8_t)(sequence >> 8), (uint8_t)sequence};
    memcpy(request + 8, payload, sizeof payload - 1);
    if (!v6) { uint16_t sum = checksum(request, sizeof request, 0); request[2] = (uint8_t)(sum >> 8); request[3] = (uint8_t)sum; }
    sent(raw, request, sizeof request, to);
}
/* What the headers on the wire said about the request, which the loopback hands to every raw socket as it hands it the reply. */
struct wire { int requests, hops, whole; };
/* Reads until the reply to `sequence` has come, through the packets group or through the sockets group; `flags` apply to the reply alone. What else ICMP brings
 * meanwhile is not the test's business. Over IPv4 a message starts with its 20-byte IP header, over IPv6 with the ICMPv6 header. */
static struct arrival answered(void *raw, int v6, int described, uint16_t sequence, uint32_t flags, struct wire *wire) {
    const dotnet_pal_socket_address from = loopback(v6 ? V6 : V4, 0); const dotnet_pal_packet_info here = {loopback_index, 0, from};
    for (int i = 0; i < 64; ++i) {
        struct arrival a;
        if (described) a = take(raw, sizeof a.data, PEEK);
        else { memset(&a, 0xAA, sizeof a); a.info = here; a.status = s->receive(raw, a.data, sizeof a.data, PEEK, &a.from, &a.received); }
        assert(a.status == DOTNET_PAL_OK);
        const uint8_t *icmp = a.data + (v6 ? 0 : 20); size_t size = a.received - (v6 ? 0 : 20);
        int ours = a.received >= (v6 ? 8u : 28u) && (v6 || (a.data[0] == 0x45 && a.data[9] == IPPROTO_ICMP)) && icmp[4] == (uint8_t)(echo_id >> 8) && icmp[5] == (uint8_t)echo_id
            && icmp[6] == (uint8_t)(sequence >> 8) && icmp[7] == (uint8_t)sequence;
        int reply = ours && icmp[0] == (v6 ? 129 : 0);
        if (ours) {
            /* Both the request and the reply went from the loopback address to the loopback address, and a raw socket's sender has no port. */
            assert(memcmp(&a.from, &from, sizeof from) == 0 && memcmp(&a.info, &here, sizeof here) == 0);
            assert(icmp[1] == 0 && size == ECHO && memcmp(icmp + 8, payload, sizeof payload - 1) == 0);
            if (!v6) assert(checksum(icmp, size, 0) == 0 && memcmp(a.data + 12, "\x7f\0\0\1\x7f\0\0\1", 8) == 0);
            else { uint8_t pseudo[40] = {[15] = 1, [31] = 1, [35] = ECHO, [39] = IPPROTO_ICMPV6}; assert(checksum(icmp, size, (uint16_t)~checksum(pseudo, sizeof pseudo, 0)) == 0); }
            if (icmp[0] == (v6 ? 128 : 8) && wire) { ++wire->requests; if (!v6) { wire->hops = a.data[8]; wire->whole = (a.data[6] & 0x40) != 0; } }
        }
        /* The message that was peeked at is taken now: the same bytes, sender and description. */
        struct arrival taken;
        if (described) taken = take(raw, sizeof taken.data, reply ? flags : 0);
        else { memset(&taken, 0xAA, sizeof taken); taken.info = here; taken.status = s->receive(raw, taken.data, sizeof taken.data, reply ? flags : 0, &taken.from, &taken.received); }
        assert(memcmp(&taken, &a, sizeof a) == 0);
        if (reply) return a;
    }
    assert(!"no echo reply");
    return (struct arrival){0};
}
/* A raw socket of the sockets group: an echo request to the loopback address, and the reply through both groups. */
static void echo(int v6) {
    uint32_t family = v6 ? V6 : V4; int fd = next_descriptor(); dotnet_pal_socket_address at = loopback(family, 0), any = {(uint16_t)family, 0, 0, {0}}, address; struct wire wire = {0, 0, 0};
    void *raw = open_socket(family, RAW);
    assert(kernel(fd, SOL_SOCKET, SO_TYPE) == SOCK_RAW && kernel(fd, SOL_SOCKET, SO_DOMAIN) == (v6 ? AF_INET6 : AF_INET) && kernel(fd, SOL_SOCKET, SO_PROTOCOL) == (v6 ? IPPROTO_ICMPV6 : IPPROTO_ICMP));
    assert(fcntl(fd, F_GETFD) & FD_CLOEXEC);
    /* A raw socket has no ports. The kernel reports its protocol number as one; the group reports none. */
    struct sockaddr_storage name; socklen_t length = sizeof name;
    assert(getsockname(fd, (struct sockaddr*)&name, &length) == 0 && ntohs(((struct sockaddr_in*)&name)->sin_port) == (v6 ? IPPROTO_ICMPV6 : IPPROTO_ICMP));
    memset(&address, 0xff, sizeof address);
    assert(s->local_address(raw, &address) == 0 && memcmp(&address, &any, sizeof any) == 0);
    assert(s->set_option(raw, INFORMATION, 1) == 0 && option(raw, INFORMATION) == 1);
    /* Ping asks a raw socket for what the network reports about its requests (a hop limit that ran out, a host nobody answers for). */
    assert(option(raw, ERRORS) == 0 && s->set_option(raw, ERRORS, 1) == 0 && option(raw, ERRORS) == 1 && kernel(fd, v6 ? IPPROTO_IPV6 : IPPROTO_IP, v6 ? IPV6_RECVERR : IP_RECVERR) == 1);
    ask(raw, v6, 1, &at);
    assert(bits(raw, READ, 5000 * MS) == READ);
    answered(raw, v6, 1, 1, 0, &wire);
    assert(wire.requests == 1);
    /* The hop limit and the fragmentation switch Ping sets show in the header of what the socket sends. A port in the target is dropped: the kernel
     * would read an IPv6 one as a protocol number and refuse it. */
    assert(s->set_option(raw, HOPS, 5) == 0 && option(raw, HOPS) == 5 && kernel(fd, v6 ? IPPROTO_IPV6 : IPPROTO_IP, v6 ? IPV6_UNICAST_HOPS : IP_TTL) == 5);
    if (v6) unsupported(raw, WHOLE);
    else assert(option(raw, WHOLE) == 0 && s->set_option(raw, WHOLE, 1) == 0 && option(raw, WHOLE) == 1 && kernel(fd, IPPROTO_IP, IP_MTU_DISCOVER) == IP_PMTUDISC_DO);
    at.port = 77;
    ask(raw, v6, 2, &at);
    answered(raw, v6, 0, 2, 0, &wire);
    assert(wire.requests == 2 && (v6 || (wire.hops == 5 && wire.whole)));
    if (!v6) {
        assert(s->set_option(raw, WHOLE, 0) == 0 && option(raw, WHOLE) == 0 && kernel(fd, IPPROTO_IP, IP_MTU_DISCOVER) == IP_PMTUDISC_DONT);
        ask(raw, v6, 3, &at);
        answered(raw, v6, 1, 3, 0, &wire);
        assert(wire.requests == 3 && wire.hops == 5 && !wire.whole);
    }
    /* bind and connect take an address and drop its port; the endpoints report none, whatever the kernel keeps. */
    at.port = 55;
    assert(s->bind(raw, &at) == 0 && s->local_address(raw, &address) == 0);
    at.port = 0;
    assert(memcmp(&address, &at, sizeof at) == 0);
    at.port = 77;
    assert(s->connect(raw, &at) == 0 && s->peer_address(raw, &address) == 0);
    at.port = 0;
    assert(memcmp(&address, &at, sizeof at) == 0);
    ask(raw, v6, 4, NULL);
    answered(raw, v6, 1, 4, 0, &wire);
    closed(raw, fd);
}
/* Whether the next datagram `sender` sends to `to` crosses the loopback with the header bit that forbids fragmenting it; `wire` sees every UDP packet with its IP header. */
static int whole_on_wire(int wire, void *sender, void *receiver, const dotnet_pal_socket_address *to) {
    uint8_t packet[128], sink[8]; size_t done;
    sent(sender, "bit", 3, to);
    assert(s->receive(receiver, sink, sizeof sink, 0, NULL, &done) == 0 && done == 3);
    for (int i = 0; i < 64; ++i) {
        assert(waits(wire));
        ssize_t n = recv(wire, packet, sizeof packet, 0);
        assert(n >= 28);
        size_t header = (size_t)(packet[0] & 0x0f) * 4;
        if ((size_t)n >= header + 8 && (packet[header + 2] << 8 | packet[header + 3]) == to->port) return (packet[6] & 0x40) != 0;
    }
    assert(!"the datagram never crossed the loopback");
    return -1;
}
/* DONT_FRAGMENT on a datagram socket. No datagram is longer than this loopback's MTU (65536, and IPv4 ends at 65535), so the effect is read from the wire when the test may look at it. */
static void fragmentation(int may_look) {
    int fd = next_descriptor(), probe = IP_PMTUDISC_PROBE; void *sender = open_socket(V4, UDP), *receiver = open_socket(V4, UDP), *six = NULL; dotnet_pal_socket_address at = loopback(V4, 0);
    assert(s->bind(receiver, &at) == 0 && s->local_address(receiver, &at) == 0);
    /* What nobody set fragments a datagram that is too long for the route, so it reads as off. */
    assert(option(sender, WHOLE) == 0 && kernel(fd, IPPROTO_IP, IP_MTU_DISCOVER) == IP_PMTUDISC_WANT);
    int wire = may_look ? socket(AF_INET, SOCK_RAW | SOCK_CLOEXEC, IPPROTO_UDP) : -1;
    assert(!may_look || wire >= 0);
    assert(s->set_option(sender, WHOLE, 1) == 0 && option(sender, WHOLE) == 1 && kernel(fd, IPPROTO_IP, IP_MTU_DISCOVER) == IP_PMTUDISC_DO);
    if (may_look) assert(whole_on_wire(wire, sender, receiver, &at) == 1);
    assert(s->set_option(sender, WHOLE, 0) == 0 && option(sender, WHOLE) == 0 && kernel(fd, IPPROTO_IP, IP_MTU_DISCOVER) == IP_PMTUDISC_DONT);
    if (may_look) assert(whole_on_wire(wire, sender, receiver, &at) == 0);
    /* PROBE, which only a caller behind the boundary's back sets, keeps datagrams whole as well. */
    assert(setsockopt(fd, IPPROTO_IP, IP_MTU_DISCOVER, &probe, sizeof probe) == 0 && option(sender, WHOLE) == 1);
    if (may_look) assert(close(wire) == 0);
    /* The kernel would take the option on an IPv6 socket; it is IPv4's. */
    if (s->create(V6, UDP, &six) == 0) { unsupported(six, WHOLE); assert(s->close(six) == 0); }
    closed(sender, fd);
    assert(s->close(receiver) == 0);
}

int main(int argc, char **argv) {
#ifdef PAL_HOST_TEST
    pal_packets_fault = argc > 1 ? atoi(argv[1]) : 0;
#else
    (void)argc; (void)argv;
#endif
    const dotnet_pal_api *api = dotnet_pal_get_api(2);
#ifdef PAL_HOST_TEST
    if (pal_packets_fault == 1) { assert(!api); puts("PACKETS malformed host rejected"); return 0; }
#endif
    assert(api && api->header.struct_size >= DOTNET_PAL_PACKETS_API_SIZE);
    assert((api->header.capabilities & DOTNET_PAL_CAP_PACKETS) && (api->header.capabilities & DOTNET_PAL_CAP_SOCKETS));
    s = &api->sockets; p = &api->packets;
    assert(p->receive && p->read_stats);
    dotnet_pal_packets_stats stats;
#ifdef PAL_HOST_TEST
    if (pal_packets_fault == 2) {
        /* The socket is real; every answer about it is one the front end has to put right. */
        void *socket = open_socket(V4, UDP); const dotnet_pal_socket_address sender = loopback(V4, 4242); dotnet_pal_packet_info clean = {1, 0, loopback(V4, 0)}, bare = {1, 0, nobody};
        struct arrival a = take(socket, 8, 0);
        assert(failed(&a, DOTNET_PAL_OS_ERROR)); /* more than fits */
        a = take(socket, 8, 0); /* a destination with a port is no destination; the rest of the answer stands */
        assert(a.status == 0 && a.received == 8 && memcmp(&a.info, &bare, sizeof bare) == 0 && memcmp(&a.from, &sender, sizeof sender) == 0);
        a = take(socket, 8, 0); /* families the boundary does not define: no destination and no sender */
        assert(a.status == 0 && a.received == 8 && memcmp(&a.info, &bare, sizeof bare) == 0 && memcmp(&a.from, &nobody, sizeof nobody) == 0);
        a = take(socket, 8, 0); /* the reserved field and the bytes an IPv4 address does not use are cleared */
        assert(a.status == 0 && a.received == 8 && memcmp(&a.info, &clean, sizeof clean) == 0 && memcmp(&a.from, &sender, sizeof sender) == 0);
        a = take(socket, 8, 0); /* a failure arrives without the outputs of the call that reported it */
        assert(failed(&a, DOTNET_PAL_WOULD_BLOCK));
        a = take(socket, 8, 0); /* a status only other groups have */
        assert(failed(&a, DOTNET_PAL_OS_ERROR));
        a = take(socket, 8, 0); /* no such status */
        assert(failed(&a, DOTNET_PAL_OS_ERROR));
        assert(s->close(socket) == 0);
        assert(p->read_stats(&stats, sizeof stats) == 0 && memcmp(&stats, &expected, sizeof stats) == 0 && stats.receive_ok == 3 && stats.rejected_or_failed == 4);
        puts("PACKETS host errors sanitized"); return 0;
    }
#endif
    interfaces();
    echo_id = (uint16_t)getpid();
    datagrams(V4, 0, V4);
    /* IPv6 is there when its loopback address can be bound. */
    void *six = NULL; dotnet_pal_socket_address at6 = loopback(V6, 0);
    int v6 = s->create(V6, UDP, &six) == 0 && s->bind(six, &at6) == 0;
    if (six) assert(s->close(six) == 0);
    if (v6) { datagrams(V6, 0, V6); datagrams(V6, 1, V4); datagrams(V6, 1, V6); }
    refused();
    other_sockets();
    unsigned rejected = validation();
    /* A raw socket is a privilege (CAP_NET_RAW): with it the echo runs, without it the group says so. A local socket speaks no ICMP either way. */
    void *handle = (void*)1; const char *raw = "allowed";
    assert(s->create(LOCAL, RAW, &handle) == INVALID && handle == NULL);
    int own = socket(AF_INET, SOCK_RAW | SOCK_CLOEXEC, IPPROTO_ICMP);
    if (own >= 0) {
        assert(close(own) == 0);
        echo(0);
        if (v6) {
            /* The kernel fact behind the dropped port: an IPv6 raw send reads it as a protocol number. */
            struct sockaddr_in6 to = {0}; to.sin6_family = AF_INET6; to.sin6_addr = in6addr_loopback; to.sin6_port = htons(77);
            own = socket(AF_INET6, SOCK_RAW | SOCK_CLOEXEC, IPPROTO_ICMPV6);
            assert(own >= 0 && sendto(own, "\x80\0\0\0\0\0\0\0", 8, 0, (struct sockaddr*)&to, sizeof to) == -1 && errno == EINVAL && close(own) == 0);
            echo(1);
        }
    } else {
        assert(errno == EPERM || errno == EACCES);
        handle = (void*)1;
        assert(s->create(V4, RAW, &handle) == DOTNET_PAL_ACCESS_DENIED && handle == NULL);
        handle = (void*)1;
        if (v6) assert(s->create(V6, RAW, &handle) == DOTNET_PAL_ACCESS_DENIED && handle == NULL);
        raw = "denied";
        puts("PACKETS note: this process may not open raw sockets (no CAP_NET_RAW); the echo and the headers on the wire were not checked");
    }
    fragmentation(own >= 0);
    /* Counters: one per answered receive, one for everything refused, and nothing else. */
    assert(p->read_stats(&stats, sizeof stats) == 0 && memcmp(&stats, &expected, sizeof stats) == 0);
    assert(stats.receive_ok >= 8 && stats.rejected_or_failed >= rejected + 7);
    printf("PACKETS PASS loopback=%u other=%s/%u ipv6=%s raw=%s received=%llu refused=%llu rejected=%u\n", loopback_index, other_name, other_index, v6 ? "checked" : "absent", raw,
        (unsigned long long)stats.receive_ok, (unsigned long long)stats.rejected_or_failed, rejected);
    return 0;
}

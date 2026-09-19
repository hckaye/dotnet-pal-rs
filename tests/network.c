/* Conformance test of the network group on Linux: the interface and address lists
 * against if_nameindex, the interface ioctls, getifaddrs and sysfs asked by the
 * test itself, two threads enumerating at once, reverse lookup against
 * getnameinfo, and multicast membership shown by real datagrams between sockets of
 * the sockets group. With PAL_HOST_TEST the same program runs against the C host
 * tables; fault 1 is a rejected table, fault 2 a provider that breaks the output
 * contracts, fault 3 a table without callbacks. */
#define _GNU_SOURCE
#include "dotnet_pal.h"
#include <arpa/inet.h>
#include <assert.h>
#include <ifaddrs.h>
#include <net/if.h>
#include <net/if_arp.h>
#include <netdb.h>
#include <netinet/in.h>
#include <netpacket/packet.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/socket.h>
#include <time.h>
#include <unistd.h>
#ifndef ARPHRD_IP6GRE
#define ARPHRD_IP6GRE 823 /* <linux/if_arp.h>; the C library's header stops before it */
#endif
#ifdef PAL_HOST_TEST
extern int pal_network_fault;
#endif
enum { V4 = DOTNET_PAL_FAMILY_IPV4, V6 = DOTNET_PAL_FAMILY_IPV6, TCP = DOTNET_PAL_SOCKET_STREAM, UDP = DOTNET_PAL_SOCKET_DATAGRAM, MOST = 1024, CAPACITY = 512 };
#define MS UINT64_C(1000000)
#define INVALID DOTNET_PAL_INVALID_ARGUMENT
#define SMALL DOTNET_PAL_BUFFER_TOO_SMALL
static const dotnet_pal_network_ops *n;
static const dotnet_pal_sockets_ops *s;
/* Every call of the group goes through one of these, so the counters can be checked to the call. */
static dotnet_pal_network_stats expected;
static unsigned rejected;
static uint32_t tally(uint32_t status, uint64_t *ok) { ++*(status == DOTNET_PAL_OK ? ok : &expected.rejected_or_failed); return status; }
static uint32_t interface_at(size_t index, dotnet_pal_network_interface *out, size_t size) { return tally(n->interface_entry(index, out, size), &expected.interface_ok); }
static uint32_t address_at(size_t index, dotnet_pal_network_address *out, size_t size) { return tally(n->address_entry(index, out, size), &expected.address_ok); }
static uint32_t lookup(const dotnet_pal_socket_address *address, uint8_t *out, size_t capacity, size_t *needed) { return tally(n->reverse_lookup(address, out, capacity, needed), &expected.lookup_ok); }
static uint32_t member(void *socket, const dotnet_pal_socket_address *group, uint32_t interface, uint32_t join) { return tally(n->membership(socket, group, interface, join), &expected.membership_ok); }
#define REJECT(call) do { assert((call) == INVALID); ++rejected; } while (0)
static int zero(const void *bytes, size_t size) { const uint8_t *b = bytes; while (size--) if (*b++) return 0; return 1; }
static uint8_t text[CAPACITY + 1];

static dotnet_pal_network_interface interfaces[MOST];
static dotnet_pal_network_address addresses[MOST];
static size_t interface_count, address_count;
static const dotnet_pal_network_interface *named(const char *name) {
    for (size_t i = 0; i < interface_count; ++i) if (strcmp((const char*)interfaces[i].name, name) == 0) return &interfaces[i];
    return NULL;
}
static uint32_t kind_of(unsigned type, const char *name, short flags) {
    char path[128];
    if (type == ARPHRD_LOOPBACK) return DOTNET_PAL_INTERFACE_LOOPBACK;
    if (type == ARPHRD_ETHER) { snprintf(path, sizeof path, "/sys/class/net/%s/wireless", name); return access(path, F_OK) == 0 ? DOTNET_PAL_INTERFACE_WIRELESS : DOTNET_PAL_INTERFACE_ETHERNET; }
    if (type == ARPHRD_PPP) return DOTNET_PAL_INTERFACE_POINT_TO_POINT;
    if (type == ARPHRD_TUNNEL || type == ARPHRD_TUNNEL6 || type == ARPHRD_SIT || type == ARPHRD_IPGRE || type == ARPHRD_IP6GRE) return DOTNET_PAL_INTERFACE_TUNNEL;
    return flags & IFF_POINTOPOINT ? DOTNET_PAL_INTERFACE_POINT_TO_POINT : DOTNET_PAL_INTERFACE_UNKNOWN;
}
static uint64_t speed_of(const char *name) {
    char path[128], line[64] = ""; long long megabits = -1;
    snprintf(path, sizeof path, "/sys/class/net/%s/speed", name);
    FILE *file = fopen(path, "r");
    if (!file) return 0;
    if (fgets(line, sizeof line, file)) megabits = atoll(line);
    fclose(file);
    return megabits > 0 ? (uint64_t)megabits * UINT64_C(1000000) : 0;
}
/* The whole interface list, and each entry against what the kernel tells this program about the same name. */
static void enumerate_interfaces(struct ifaddrs *list) {
    dotnet_pal_network_interface entry;
    for (interface_count = 0; interface_count < MOST; ++interface_count) {
        memset(&entry, 0xAA, sizeof entry);
        uint32_t status = interface_at(interface_count, &entry, sizeof entry);
        if (status == DOTNET_PAL_NOT_FOUND) break;
        assert(status == DOTNET_PAL_OK);
        interfaces[interface_count] = entry;
    }
    assert(interface_count >= 1 && interface_count < MOST && zero(&entry, sizeof entry));
    memset(&entry, 0xAA, sizeof entry);
    assert(interface_at(interface_count + 1, &entry, sizeof entry) == DOTNET_PAL_NOT_FOUND && zero(&entry, sizeof entry));
    assert(interface_at(SIZE_MAX, &entry, sizeof entry) == DOTNET_PAL_NOT_FOUND);
    /* The same names with the same indexes as if_nameindex, each once. */
    struct if_nameindex *names = if_nameindex(); size_t known = 0;
    assert(names);
    for (; names[known].if_index != 0; ++known) {
        const dotnet_pal_network_interface *found = named(names[known].if_name);
        assert(found && found->index == names[known].if_index);
    }
    assert(known == interface_count);
    if_freenameindex(names);
    int probe = socket(AF_INET, SOCK_DGRAM | SOCK_CLOEXEC, 0);
    assert(probe >= 0);
    for (size_t i = 0; i < interface_count; ++i) {
        const dotnet_pal_network_interface *found = &interfaces[i]; const char *name = (const char*)found->name; struct ifreq request;
        size_t length = strlen(name);
        assert(length >= 1 && length < IF_NAMESIZE && zero(found->name + length, sizeof found->name - length) && found->index == if_nametoindex(name));
        for (size_t other = 0; other < i; ++other) assert(interfaces[other].index != found->index && strcmp((const char*)interfaces[other].name, name) != 0);
        memset(&request, 0, sizeof request); snprintf(request.ifr_name, sizeof request.ifr_name, "%s", name);
        assert(ioctl(probe, SIOCGIFFLAGS, &request) == 0);
        short flags = request.ifr_flags;
        assert(found->state == ((flags & IFF_UP) && (flags & IFF_RUNNING) ? DOTNET_PAL_LINK_UP : !(flags & IFF_UP) ? DOTNET_PAL_LINK_DOWN : DOTNET_PAL_LINK_UNKNOWN));
        assert(found->flags == (flags & IFF_MULTICAST ? DOTNET_PAL_INTERFACE_MULTICAST : 0u));
        assert(ioctl(probe, SIOCGIFMTU, &request) == 0 && found->mtu == (uint32_t)request.ifr_mtu);
        assert(ioctl(probe, SIOCGIFHWADDR, &request) == 0 && found->kind == kind_of(request.ifr_hwaddr.sa_family, name, flags));
        assert(found->speed_bps == speed_of(name));
        /* The hardware address: the link-layer entry of getifaddrs has its length, the ioctl its first bytes. */
        const struct sockaddr_ll *link = NULL;
        for (struct ifaddrs *a = list; a && !link; a = a->ifa_next) if (a->ifa_addr && a->ifa_addr->sa_family == AF_PACKET && strcmp(a->ifa_name, name) == 0) link = (const struct sockaddr_ll*)a->ifa_addr;
        assert(link && link->sll_hatype == request.ifr_hwaddr.sa_family && (unsigned)link->sll_ifindex == found->index);
        uint32_t carried = link->sll_halen <= sizeof found->hardware_address ? link->sll_halen : 0;
        assert(found->hardware_address_length == carried && memcmp(found->hardware_address, link->sll_addr, carried) == 0);
        assert(zero(found->hardware_address + carried, sizeof found->hardware_address - carried));
        if (carried == 6) assert(memcmp(found->hardware_address, request.ifr_hwaddr.sa_data, 6) == 0);
    }
    close(probe);
    const dotnet_pal_network_interface *lo = named("lo");
    assert(lo && lo->kind == DOTNET_PAL_INTERFACE_LOOPBACK && lo->state == DOTNET_PAL_LINK_UP);
}
static uint32_t ones(const uint8_t *mask, size_t size) { uint32_t bits = 0; for (size_t i = 0; i < size; ++i) for (int bit = 0; bit < 8; ++bit) bits += (mask[i] >> bit) & 1; return bits; }
/* The address list as a set: every IPv4 and IPv6 entry of getifaddrs, once, with its prefix length, interface and scope. */
static int enumerate_addresses(struct ifaddrs *list) {
    static dotnet_pal_network_address own[MOST]; static int used[MOST]; size_t own_count = 0; int six_loopback = 0; dotnet_pal_network_address entry;
    for (struct ifaddrs *a = list; a; a = a->ifa_next) {
        if (!a->ifa_addr || (a->ifa_addr->sa_family != AF_INET && a->ifa_addr->sa_family != AF_INET6)) continue;
        char name[IF_NAMESIZE]; snprintf(name, sizeof name, "%s", a->ifa_name); name[strcspn(name, ":")] = 0;
        dotnet_pal_network_address *o = &own[own_count++];
        assert(own_count < MOST && a->ifa_netmask);
        memset(o, 0, sizeof *o);
        o->interface_index = if_nametoindex(name);
        if (a->ifa_addr->sa_family == AF_INET) {
            o->address.family = V4; memcpy(o->address.address, &((struct sockaddr_in*)a->ifa_addr)->sin_addr, 4);
            o->prefix_length = ones((const uint8_t*)&((struct sockaddr_in*)a->ifa_netmask)->sin_addr, 4);
        } else {
            o->address.family = V6; memcpy(o->address.address, &((struct sockaddr_in6*)a->ifa_addr)->sin6_addr, 16);
            o->prefix_length = ones((const uint8_t*)&((struct sockaddr_in6*)a->ifa_netmask)->sin6_addr, 16);
            if (o->address.address[0] == 0xfe && (o->address.address[1] & 0xc0) == 0x80) o->address.scope = o->interface_index;
        }
    }
    for (address_count = 0; address_count < MOST; ++address_count) {
        memset(&entry, 0xAA, sizeof entry);
        uint32_t status = address_at(address_count, &entry, sizeof entry);
        if (status == DOTNET_PAL_NOT_FOUND) break;
        assert(status == DOTNET_PAL_OK && entry.address.port == 0);
        addresses[address_count] = entry;
        size_t match = 0;
        while (match < own_count && (used[match] || memcmp(&own[match], &entry, sizeof entry) != 0)) ++match;
        assert(match < own_count);
        used[match] = 1;
        int found = 0;
        for (size_t i = 0; i < interface_count; ++i) found += interfaces[i].index == entry.interface_index;
        assert(found == 1);
    }
    assert(address_count == own_count && zero(&entry, sizeof entry));
    memset(&entry, 0xAA, sizeof entry);
    assert(address_at(address_count + 1, &entry, sizeof entry) == DOTNET_PAL_NOT_FOUND && zero(&entry, sizeof entry) && address_at(SIZE_MAX, &entry, sizeof entry) == DOTNET_PAL_NOT_FOUND);
    /* Loopback has 127.0.0.1/8, and ::1/128 where this machine has IPv6 switched on. */
    dotnet_pal_network_address v4 = {named("lo")->index, 8, {V4, 0, 0, {127, 0, 0, 1}}}, v6 = {named("lo")->index, 128, {V6, 0, 0, {0}}};
    v6.address.address[15] = 1;
    int has_v4 = 0, has_v6 = 0;
    for (size_t i = 0; i < address_count; ++i) { has_v4 += memcmp(&addresses[i], &v4, sizeof v4) == 0; has_v6 += memcmp(&addresses[i], &v6, sizeof v6) == 0; }
    for (size_t i = 0; i < own_count; ++i) six_loopback += memcmp(&own[i], &v6, sizeof v6) == 0;
    assert(has_v4 == 1 && has_v6 == six_loopback);
    return six_loopback;
}
/* Both lists again, entry for entry what the first thread saw; index 0 takes a new snapshot while the other thread is in the middle of its own. */
struct pass { uint64_t interface_ok, address_ok, failed; };
static void *enumerate_again(void *arg) {
    struct pass *counted = arg;
    for (int round = 0; round < 50; ++round) {
        dotnet_pal_network_interface one; dotnet_pal_network_address other;
        for (size_t i = 0; i < interface_count; ++i) { assert(n->interface_entry(i, &one, sizeof one) == 0 && memcmp(&one, &interfaces[i], sizeof one) == 0); ++counted->interface_ok; }
        assert(n->interface_entry(interface_count, &one, sizeof one) == DOTNET_PAL_NOT_FOUND);
        for (size_t i = 0; i < address_count; ++i) { assert(n->address_entry(i, &other, sizeof other) == 0 && memcmp(&other, &addresses[i], sizeof other) == 0); ++counted->address_ok; }
        assert(n->address_entry(address_count, &other, sizeof other) == DOTNET_PAL_NOT_FOUND);
        counted->failed += 2;
    }
    return NULL;
}
/* The answer is `expected_text`: whole with its NUL where it fits and cleared behind it, the length alone and a cleared buffer where it does not. */
static void delivers(const dotnet_pal_socket_address *address, const char *expected_text) {
    size_t length = strlen(expected_text) + 1, needed = 7;
    assert(length >= 2 && length <= 256);
    memset(text, 0xAA, sizeof text);
    assert(lookup(address, text, CAPACITY, &needed) == 0 && needed == length && memcmp(text, expected_text, length) == 0 && zero(text + length, CAPACITY - length) && text[CAPACITY] == 0xAA);
    memset(text, 0xAA, sizeof text); needed = 7;
    assert(lookup(address, text, length, &needed) == 0 && needed == length && memcmp(text, expected_text, length) == 0 && text[length] == 0xAA);
    memset(text, 0xAA, sizeof text); needed = 7;
    assert(lookup(address, text, length - 1, &needed) == SMALL && needed == length && zero(text, length - 1) && text[length - 1] == 0xAA);
    needed = 7;
    assert(lookup(address, NULL, 0, &needed) == SMALL && needed == length);
}
static void refuses(const dotnet_pal_socket_address *address, uint32_t status) {
    size_t needed = 7;
    memset(text, 0xAA, sizeof text);
    assert(lookup(address, text, CAPACITY, &needed) == status && needed == 0 && zero(text, CAPACITY) && text[CAPACITY] == 0xAA);
}
/* What getnameinfo says about the same address decides what the group has to say: the name, NOT_FOUND or TIMEOUT. */
static const char *reverse(const dotnet_pal_socket_address *address, char *host) {
    struct sockaddr_in v4; struct sockaddr_in6 v6; int code;
    memset(&v4, 0, sizeof v4); memset(&v6, 0, sizeof v6);
    v4.sin_family = AF_INET; memcpy(&v4.sin_addr, address->address, 4);
    v6.sin6_family = AF_INET6; memcpy(&v6.sin6_addr, address->address, 16); v6.sin6_scope_id = address->scope;
    code = address->family == V4 ? getnameinfo((struct sockaddr*)&v4, sizeof v4, host, NI_MAXHOST, NULL, 0, NI_NAMEREQD) : getnameinfo((struct sockaddr*)&v6, sizeof v6, host, NI_MAXHOST, NULL, 0, NI_NAMEREQD);
    if (code == 0) { delivers(address, host); return host; }
    if (code == EAI_NONAME) { refuses(address, DOTNET_PAL_NOT_FOUND); return "NOT_FOUND"; }
    if (code == EAI_AGAIN) { refuses(address, DOTNET_PAL_TIMEOUT); return "TIMEOUT"; }
    printf("NETWORK note: getnameinfo answered %d (%s) here, no expectation for that address\n", code, gai_strerror(code));
    return "unchecked";
}

static void *open_socket(uint32_t family, uint32_t kind) { void *socket = NULL; assert(s->create(family, kind, &socket) == 0 && socket); return socket; }
static uint16_t bind_any(void *socket, uint32_t family) {
    dotnet_pal_socket_address any = {(uint16_t)family, 0, 0, {0}}, at;
    assert(s->bind(socket, &any) == 0 && s->local_address(socket, &at) == 0 && at.port != 0);
    return at.port;
}
static int readable(void *socket, uint64_t timeout_ns) {
    dotnet_pal_poll_entry entry = {socket, DOTNET_PAL_POLL_READ, 0}; size_t ready = 7;
    assert(s->poll(&entry, 1, timeout_ns, DOTNET_PAL_NO_CHANNEL, &ready) == 0);
    return ready == 1 && (entry.triggered & DOTNET_PAL_POLL_READ);
}
static uint64_t now(void) { struct timespec ts; assert(clock_gettime(CLOCK_MONOTONIC, &ts) == 0); return (uint64_t)ts.tv_sec * UINT64_C(1000000000) + (uint64_t)ts.tv_nsec; }
/* Whether a datagram with this text reaches the socket in time. Anything else that arrives is read and dropped: a bridge was measured
 * to hand an IPv6 multicast datagram to its sender's machine a second time, which must not pass for the datagram sent after it. */
static int arrives(void *socket, const char *wanted, uint64_t timeout_ns, dotnet_pal_socket_address *from) {
    uint8_t data[16]; size_t done = 7; uint64_t deadline = now() + timeout_ns, current;
    while ((current = now()) < deadline && readable(socket, deadline - current)) {
        assert(s->receive(socket, data, sizeof data, 0, from, &done) == 0);
        if (done == strlen(wanted) && memcmp(data, wanted, done) == 0) return 1;
    }
    return 0;
}
/* An interface of the group's own list that is up, carries multicast, is no loopback and has an address of `family`; 0 when there is none. */
static uint32_t multicast_interface(uint32_t family) {
    for (size_t i = 0; i < interface_count; ++i) {
        const dotnet_pal_network_interface *found = &interfaces[i];
        if (!(found->flags & DOTNET_PAL_INTERFACE_MULTICAST) || found->state != DOTNET_PAL_LINK_UP || found->kind == DOTNET_PAL_INTERFACE_LOOPBACK) continue;
        for (size_t a = 0; a < address_count; ++a) if (addresses[a].interface_index == found->index && addresses[a].address.family == family) return found->index;
    }
    return 0;
}
/* A datagram to `group` reaches a member through `interface`, and no longer once the socket has left. */
static void traffic(uint32_t family, dotnet_pal_socket_address group, uint32_t interface) {
    void *receiver = open_socket(family, UDP), *sender = open_socket(family, UDP); size_t done = 7; dotnet_pal_socket_address from, at;
    group.port = bind_any(receiver, family);
    if (family == V6) group.scope = interface;
    assert(s->set_option(sender, DOTNET_PAL_SOCKET_MULTICAST_LOOPBACK, 1) == 0 && s->set_option(sender, DOTNET_PAL_SOCKET_MULTICAST_INTERFACE, interface) == 0);
    assert(member(receiver, &group, interface, 1) == 0);
    assert(s->send(sender, (const uint8_t*)"first", 5, &group, &done) == 0 && done == 5 && arrives(receiver, "first", 5000 * MS, &from));
    assert(s->local_address(sender, &at) == 0 && from.port == at.port && from.family == family);
    assert(member(receiver, &group, interface, 1) == DOTNET_PAL_ADDRESS_IN_USE);
    assert(member(receiver, &group, interface, 0) == 0);
    assert(s->send(sender, (const uint8_t*)"second", 6, &group, &done) == 0 && done == 6 && !arrives(receiver, "second", 300 * MS, &from));
    assert(member(receiver, &group, interface, 0) == DOTNET_PAL_ADDRESS_NOT_AVAILABLE);
    assert(s->close(receiver) == 0 && s->close(sender) == 0);
}
/* The statuses of joining and leaving on one datagram socket, without traffic. */
static void statuses(uint32_t family, dotnet_pal_socket_address group, dotnet_pal_socket_address other_family, uint32_t interface) {
    void *datagram = open_socket(family, UDP), *stream = open_socket(family, TCP);
    /* An IPv6 socket that carries IPv6 alone, so that an IPv4 group is not its own. */
    if (family == V6) assert(s->set_option(datagram, DOTNET_PAL_SOCKET_IPV6_ONLY, 1) == 0);
    (void)bind_any(datagram, family);
    assert(member(datagram, &group, interface, 0) == DOTNET_PAL_ADDRESS_NOT_AVAILABLE); /* a group never joined */
    assert(member(datagram, &group, interface, 1) == 0 && member(datagram, &group, interface, 1) == DOTNET_PAL_ADDRESS_IN_USE);
    assert(member(datagram, &group, interface, 0) == 0 && member(datagram, &group, interface, 0) == DOTNET_PAL_ADDRESS_NOT_AVAILABLE);
    assert(member(datagram, &group, 999999, 1) == DOTNET_PAL_NOT_FOUND && member(datagram, &group, UINT32_MAX, 1) == DOTNET_PAL_NOT_FOUND);
    /* A stream socket has no groups, and a group of a family the socket does not carry is not this socket's. */
    assert(member(stream, &group, interface, 1) == INVALID && member(datagram, &other_family, interface, 1) == INVALID);
    assert(s->close(datagram) == 0 && s->close(stream) == 0);
}
/* An IPv6 socket that carries both families joins an IPv4 group: the datagram of an IPv4 sender arrives, from the sender's address
 * mapped into IPv6, and no longer once the socket has left. */
static void dual(dotnet_pal_socket_address group, uint32_t interface) {
    static const uint8_t mapped[12] = {0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff};
    void *receiver = open_socket(V6, UDP), *sender = open_socket(V4, UDP), *only = open_socket(V6, UDP); size_t done = 7, own = 0; dotnet_pal_socket_address from, at;
    assert(s->set_option(receiver, DOTNET_PAL_SOCKET_IPV6_ONLY, 0) == 0 && s->set_option(only, DOTNET_PAL_SOCKET_IPV6_ONLY, 1) == 0);
    group.port = bind_any(receiver, V6); (void)bind_any(only, V6);
    assert(s->set_option(sender, DOTNET_PAL_SOCKET_MULTICAST_LOOPBACK, 1) == 0 && s->set_option(sender, DOTNET_PAL_SOCKET_MULTICAST_INTERFACE, interface) == 0);
    assert(member(receiver, &group, interface, 0) == DOTNET_PAL_ADDRESS_NOT_AVAILABLE && member(receiver, &group, interface, 1) == 0);
    assert(s->send(sender, (const uint8_t*)"mapped", 6, &group, &done) == 0 && done == 6 && arrives(receiver, "mapped", 5000 * MS, &from));
    assert(s->local_address(sender, &at) == 0 && from.family == V6 && from.port == at.port && memcmp(from.address, mapped, sizeof mapped) == 0);
    for (size_t a = 0; a < address_count; ++a) own += addresses[a].interface_index == interface && addresses[a].address.family == V4 && memcmp(addresses[a].address.address, from.address + 12, 4) == 0;
    assert(own == 1);
    assert(member(receiver, &group, interface, 1) == DOTNET_PAL_ADDRESS_IN_USE && member(receiver, &group, interface, 0) == 0);
    assert(s->send(sender, (const uint8_t*)"after", 5, &group, &done) == 0 && done == 5 && !arrives(receiver, "after", 300 * MS, &from));
    assert(member(receiver, &group, interface, 0) == DOTNET_PAL_ADDRESS_NOT_AVAILABLE);
    /* The kernel would let an IPv6-only socket join and deliver nothing to it. */
    assert(member(only, &group, interface, 1) == INVALID && member(only, &group, interface, 0) == INVALID);
    assert(s->close(receiver) == 0 && s->close(sender) == 0 && s->close(only) == 0);
}
static void validation(void) {
    dotnet_pal_network_interface one; dotnet_pal_network_address other; size_t needed = 7; uint8_t small[8];
    dotnet_pal_socket_address local = {V4, 0, 0, {127, 0, 0, 1}}, bad = local, group = {V4, 0, 0, {239, 255, 77, 77}}, unicast6 = {V6, 0, 0, {0}}, none = {0, 0, 0, {239, 255, 77, 77}};
    void *socket = open_socket(V4, UDP);
    bad.family = 9; unicast6.address[15] = 1;
    REJECT(interface_at(0, NULL, sizeof one)); REJECT(interface_at(0, &one, sizeof one - 1)); REJECT(interface_at(0, (dotnet_pal_network_interface*)((uintptr_t)&one + 1), sizeof one));
    REJECT(address_at(0, NULL, sizeof other)); REJECT(address_at(0, &other, sizeof other - 1)); REJECT(address_at(0, (dotnet_pal_network_address*)((uintptr_t)&other + 1), sizeof other));
    REJECT(lookup(&local, small, sizeof small, NULL)); REJECT(lookup(&local, NULL, 1, &needed)); REJECT(lookup(&local, small, SIZE_MAX, &needed));
    needed = 7; REJECT(lookup(NULL, small, sizeof small, &needed)); assert(needed == 0);
    needed = 7; REJECT(lookup(&bad, small, sizeof small, &needed)); assert(needed == 0);
    REJECT(lookup((dotnet_pal_socket_address*)((uintptr_t)&local + 1), small, sizeof small, &needed)); REJECT(lookup(&local, small, sizeof small, (size_t*)((uintptr_t)&needed + 1)));
    /* A group is an address of 224.0.0.0/4 or ff00::/8, join is 0 or 1, and both pointers are real. */
    REJECT(member(NULL, &group, 0, 1)); REJECT(member(socket, NULL, 0, 1)); REJECT(member(socket, &group, 0, 2)); REJECT(member(socket, (dotnet_pal_socket_address*)((uintptr_t)&group + 1), 0, 1));
    REJECT(member(socket, &local, 0, 1)); REJECT(member(socket, &unicast6, 0, 1)); REJECT(member(socket, &none, 0, 1)); REJECT(member(socket, &bad, 0, 0));
    assert(n->read_stats(NULL, sizeof expected) == INVALID && n->read_stats(&(dotnet_pal_network_stats){0}, sizeof expected - 1) == INVALID);
    assert(s->close(socket) == 0);
}
int main(int argc, char **argv) {
#ifdef PAL_HOST_TEST
    pal_network_fault = argc > 1 ? atoi(argv[1]) : 0;
#else
    (void)argc; (void)argv;
#endif
    const dotnet_pal_api *api = dotnet_pal_get_api(2);
#ifdef PAL_HOST_TEST
    if (pal_network_fault == 1) { assert(!api); puts("NETWORK malformed host rejected"); return 0; }
#endif
    assert(api && api->header.struct_size >= DOTNET_PAL_NETWORK_API_SIZE);
    assert((api->header.capabilities & DOTNET_PAL_CAP_NETWORK) && (api->header.capabilities & DOTNET_PAL_CAP_SOCKETS));
    n = &api->network; s = &api->sockets;
    assert(n->interface_entry && n->address_entry && n->reverse_lookup && n->membership && n->read_stats);
    dotnet_pal_network_stats stats; dotnet_pal_socket_address localhost = {V4, 0, 0, {127, 0, 0, 1}}, group = {V4, 0, 0, {239, 255, 77, 77}}, group6 = {V6, 0, 0, {0xff, 0x02}};
    group6.address[14] = 0x77; group6.address[15] = 0x77;
#ifdef PAL_HOST_TEST
    if (pal_network_fault == 2 || pal_network_fault == 3) {
        dotnet_pal_network_interface one; dotnet_pal_network_address other; void *socket = open_socket(V4, UDP);
        uint32_t every = pal_network_fault == 3 ? DOTNET_PAL_UNSUPPORTED : DOTNET_PAL_OS_ERROR;
        /* Fault 2: an index of 0, no name, no terminator, kind 9, state 7, flag 0x80, nine address bytes, status 99 and two statuses of other calls. */
        for (int i = 0; i < (pal_network_fault == 3 ? 1 : 10); ++i) { memset(&one, 0xAA, sizeof one); assert(interface_at(0, &one, sizeof one) == every && zero(&one, sizeof one)); }
        /* No interface, family 9, a port, /33, /129, status 99 and a status of another call. */
        for (int i = 0; i < (pal_network_fault == 3 ? 1 : 7); ++i) { memset(&other, 0xAA, sizeof other); assert(address_at(0, &other, sizeof other) == every && zero(&other, sizeof other)); }
        /* No terminator, one inside, 300 bytes, a need of 257, an empty text, no length, status 99 and a status of another call. */
        for (int i = 0; i < (pal_network_fault == 3 ? 1 : 8); ++i) refuses(&localhost, every);
        for (int i = 0; i < (pal_network_fault == 3 ? 1 : 4); ++i) assert(member(socket, &group, 0, 1) == every);
        if (pal_network_fault == 2) {
            /* The longest prefix is a good answer; the end and a failure arrive without the outputs of the call that reported them. */
            assert(address_at(0, &other, sizeof other) == 0 && other.prefix_length == 128 && other.address.family == V6);
            memset(&one, 0xAA, sizeof one); memset(&other, 0xAA, sizeof other);
            assert(interface_at(0, &one, sizeof one) == DOTNET_PAL_NOT_FOUND && zero(&one, sizeof one) && address_at(0, &other, sizeof other) == DOTNET_PAL_NOT_FOUND && zero(&other, sizeof other));
            refuses(&localhost, DOTNET_PAL_TIMEOUT); refuses(&localhost, DOTNET_PAL_NOT_FOUND);
        }
        assert(s->close(socket) == 0 && n->read_stats(&stats, sizeof stats) == 0 && memcmp(&stats, &expected, sizeof stats) == 0);
        assert(stats.rejected_or_failed == (pal_network_fault == 3 ? 4u : 33u) && stats.address_ok == (pal_network_fault == 3 ? 0u : 1u) && stats.interface_ok + stats.lookup_ok + stats.membership_ok == 0);
        puts(pal_network_fault == 3 ? "NETWORK absent host callbacks unsupported" : "NETWORK host errors sanitized"); return 0;
    }
#endif
    struct ifaddrs *list = NULL;
    assert(getifaddrs(&list) == 0);
    enumerate_interfaces(list);
    int six_loopback = enumerate_addresses(list);
    freeifaddrs(list);
    pthread_t threads[2]; struct pass passes[2] = {{0, 0, 0}, {0, 0, 0}};
    for (int i = 0; i < 2; ++i) assert(pthread_create(&threads[i], NULL, enumerate_again, &passes[i]) == 0);
    for (int i = 0; i < 2; ++i) { assert(pthread_join(threads[i], NULL) == 0); expected.interface_ok += passes[i].interface_ok; expected.address_ok += passes[i].address_ok; expected.rejected_or_failed += passes[i].failed; }

    /* Reverse lookup: loopback has a name wherever a hosts file names it; 192.0.2.1 (TEST-NET-1) has none. */
    char host[NI_MAXHOST], unnamed[NI_MAXHOST], host6[NI_MAXHOST] = "-"; dotnet_pal_socket_address test_net = {V4, 0, 0, {192, 0, 2, 1}}, loopback6 = {V6, 0, 0, {0}};
    loopback6.address[15] = 1;
    const char *name = reverse(&localhost, host), *no_name = reverse(&test_net, unnamed), *name6 = six_loopback ? reverse(&loopback6, host6) : "-";
    /* A port and the bytes an IPv4 address does not use are not part of the question. */
    dotnet_pal_socket_address noisy = {V4, 4242, 7, {127, 0, 0, 1, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9}};
    if (name == host) delivers(&noisy, host);

    /* Membership, on an interface chosen by what the group itself reports. */
    void *probe = NULL; int six = s->create(V6, UDP, &probe) == 0; int six_traffic = 0;
    if (six) assert(s->close(probe) == 0);
    uint32_t interface = multicast_interface(V4), interface6 = six ? multicast_interface(V6) : 0;
    dotnet_pal_socket_address never = group, both = group; never.address[3] = 99; both.address[3] = 79;
    if (interface) {
        traffic(V4, group, interface);
        statuses(V4, never, group6, interface);
        /* Interface 0 is the target's choice, where it has one (a route for the group). */
        void *socket = open_socket(V4, UDP);
        uint32_t chosen = member(socket, &group, 0, 1);
        assert(chosen == 0 || chosen == DOTNET_PAL_NOT_FOUND);
        if (chosen == 0) assert(member(socket, &group, 0, 0) == 0);
        assert(s->close(socket) == 0);
        if (six) { statuses(V6, group6, group, interface6 ? interface6 : interface); dual(both, interface); }
        if (interface6) { traffic(V6, group6, interface6); six_traffic = 1; }
        else if (six) puts("NETWORK note: no multicast interface with an IPv6 address here, IPv6 group traffic is skipped (the statuses are checked)");
    } else puts("NETWORK note: no interface that is up, carries multicast and has an IPv4 address; the membership checks are skipped");
    validation();
    assert(n->read_stats(&stats, sizeof stats) == 0 && memcmp(&stats, &expected, sizeof stats) == 0);
    assert(stats.rejected_or_failed >= rejected + 4 && stats.interface_ok == 101 * interface_count && stats.address_ok == 101 * address_count);
    const dotnet_pal_network_interface *used = NULL;
    for (size_t i = 0; i < interface_count; ++i) if (interfaces[i].index == interface) used = &interfaces[i];
    printf("NETWORK PASS interfaces=%zu addresses=%zu loopback=%s ipv6_loopback=%s test_net=%s multicast=%s(%u) mtu=%u speed_mbps=%llu ipv6_sockets=%d ipv6_traffic=%d ipv4_group_on_ipv6_socket=%d lookups=%llu memberships=%llu rejected=%u\n",
        interface_count, address_count, name, name6, no_name, used ? (const char*)used->name : "-", interface, used ? used->mtu : 0, used ? (unsigned long long)(used->speed_bps / 1000000) : 0,
        six, six_traffic, six && interface, (unsigned long long)stats.lookup_ok, (unsigned long long)stats.membership_ok, rejected);
    return 0;
}

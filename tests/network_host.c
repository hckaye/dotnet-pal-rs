/* Independent Linux reference provider for the host-network conformance suite. It
 * asks other sources than the Linux provider: if_nameindex and sysfs for the
 * interfaces (type, address, MTU, speed), the flags ioctl for the link state, an
 * rtnetlink dump for the addresses, gethostbyaddr_r for the reverse lookup and the
 * protocol-independent MCAST_JOIN_GROUP / MCAST_LEAVE_GROUP for membership, on the
 * descriptors of the sockets reference provider. Nothing is cached: every call
 * asks again. Fault 1 offers a table without the capability bit; fault 2 breaks
 * the output contracts so the front end's sanitizing is observable; fault 3
 * withholds every callback. */
#define _GNU_SOURCE
#include "dotnet_pal.h"
#include <errno.h>
#include <linux/netlink.h>
#include <linux/rtnetlink.h>
#include <net/if.h>
#include <net/if_arp.h>
#include <netdb.h>
#include <netinet/in.h>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/socket.h>
#include <unistd.h>
#ifndef ARPHRD_IP6GRE
#define ARPHRD_IP6GRE 823 /* <linux/if_arp.h>; the C library's header stops before it */
#endif
int pal_network_fault;
int pal_sockets_host_descriptor(void *socket);
/* The first line of /sys/class/net/<name>/<leaf>; 0 when there is none to read. */
static int attribute(const char *name, const char *leaf, char *line, size_t size) {
    char path[128];
    snprintf(path, sizeof path, "/sys/class/net/%s/%s", name, leaf);
    FILE *file = fopen(path, "r");
    if (!file) return 0;
    char *got = fgets(line, (int)size, file);
    fclose(file);
    if (!got) return 0;
    line[strcspn(line, "\n")] = 0;
    return 1;
}
static uint32_t kind_of(unsigned long type, const char *name, short flags) {
    char path[128];
    switch (type) {
    case ARPHRD_LOOPBACK: return DOTNET_PAL_INTERFACE_LOOPBACK;
    case ARPHRD_ETHER:
        snprintf(path, sizeof path, "/sys/class/net/%s/wireless", name);
        return access(path, F_OK) == 0 ? DOTNET_PAL_INTERFACE_WIRELESS : DOTNET_PAL_INTERFACE_ETHERNET;
    case ARPHRD_PPP: return DOTNET_PAL_INTERFACE_POINT_TO_POINT;
    case ARPHRD_TUNNEL: case ARPHRD_TUNNEL6: case ARPHRD_SIT: case ARPHRD_IPGRE: case ARPHRD_IP6GRE: return DOTNET_PAL_INTERFACE_TUNNEL;
    default: return flags & IFF_POINTOPOINT ? DOTNET_PAL_INTERFACE_POINT_TO_POINT : DOTNET_PAL_INTERFACE_UNKNOWN;
    }
}
static uint32_t honest_interface(size_t index, dotnet_pal_network_interface *out) {
    struct if_nameindex *list = if_nameindex();
    if (!list) return errno == ENOMEM || errno == ENOBUFS ? DOTNET_PAL_OUT_OF_MEMORY : DOTNET_PAL_OS_ERROR;
    size_t count = 0;
    while (list[count].if_index != 0) ++count;
    if (index >= count) { if_freenameindex(list); return DOTNET_PAL_NOT_FOUND; }
    char name[IF_NAMESIZE], line[128];
    unsigned number = list[index].if_index;
    snprintf(name, sizeof name, "%s", list[index].if_name);
    if_freenameindex(list);
    memset(out, 0, sizeof *out);
    out->index = number;
    snprintf((char*)out->name, sizeof out->name, "%s", name);
    struct ifreq request; memset(&request, 0, sizeof request);
    snprintf(request.ifr_name, sizeof request.ifr_name, "%s", name);
    int probe = socket(AF_INET, SOCK_DGRAM | SOCK_CLOEXEC, 0);
    if (probe < 0) return DOTNET_PAL_OS_ERROR;
    int asked = ioctl(probe, SIOCGIFFLAGS, &request);
    close(probe);
    if (asked != 0) return DOTNET_PAL_OS_ERROR;
    short flags = request.ifr_flags;
    out->state = (flags & IFF_UP) && (flags & IFF_RUNNING) ? DOTNET_PAL_LINK_UP : !(flags & IFF_UP) ? DOTNET_PAL_LINK_DOWN : DOTNET_PAL_LINK_UNKNOWN;
    out->flags = flags & IFF_MULTICAST ? DOTNET_PAL_INTERFACE_MULTICAST : 0;
    out->kind = kind_of(attribute(name, "type", line, sizeof line) ? strtoul(line, NULL, 10) : 0xFFFF, name, flags);
    if (attribute(name, "mtu", line, sizeof line)) out->mtu = (uint32_t)strtoul(line, NULL, 10);
    /* Megabits per second; -1 is a link that does not know, and loopback has no file to read. */
    if (attribute(name, "speed", line, sizeof line) && line[0] != '-') out->speed_bps = strtoull(line, NULL, 10) * UINT64_C(1000000);
    /* "aa:bb:cc:dd:ee:ff": as many pairs as addr_len says; an address longer than the boundary carries is none. */
    unsigned long length = attribute(name, "addr_len", line, sizeof line) ? strtoul(line, NULL, 10) : 0;
    if (length <= sizeof out->hardware_address && attribute(name, "address", line, sizeof line) && strlen(line) == (length ? length * 3 - 1 : 0)) {
        for (unsigned long i = 0; i < length; ++i) out->hardware_address[i] = (uint8_t)strtoul(line + i * 3, NULL, 16);
        out->hardware_address_length = (uint32_t)length;
    }
    return DOTNET_PAL_OK;
}
/* The index-th address of an RTM_GETADDR dump: IFA_LOCAL where the kernel sends one (IFA_ADDRESS is then the peer), IFA_ADDRESS otherwise. */
static uint32_t honest_address(size_t index, dotnet_pal_network_address *out) {
    static char reply[65536]; /* one dump at a time reads into it, under `dump_lock` */
    static pthread_mutex_t dump_lock = PTHREAD_MUTEX_INITIALIZER;
    struct { struct nlmsghdr header; struct ifaddrmsg body; } request;
    int fd = socket(AF_NETLINK, SOCK_RAW | SOCK_CLOEXEC, NETLINK_ROUTE);
    if (fd < 0) return DOTNET_PAL_OS_ERROR;
    memset(&request, 0, sizeof request);
    request.header.nlmsg_len = NLMSG_LENGTH(sizeof request.body); request.header.nlmsg_type = RTM_GETADDR;
    request.header.nlmsg_flags = NLM_F_REQUEST | NLM_F_DUMP; request.header.nlmsg_seq = 1;
    if (send(fd, &request, request.header.nlmsg_len, 0) < 0) { close(fd); return DOTNET_PAL_OS_ERROR; }
    uint32_t status = DOTNET_PAL_NOT_FOUND; int done = 0; size_t seen = 0;
    pthread_mutex_lock(&dump_lock);
    while (!done) {
        ssize_t size = recv(fd, reply, sizeof reply, 0);
        if (size < 0 && errno == EINTR) continue;
        if (size <= 0) { status = DOTNET_PAL_OS_ERROR; break; }
        unsigned left = (unsigned)size;
        for (struct nlmsghdr *header = (struct nlmsghdr*)reply; NLMSG_OK(header, left); header = NLMSG_NEXT(header, left)) {
            if (header->nlmsg_type == NLMSG_DONE) { done = 1; break; }
            if (header->nlmsg_type == NLMSG_ERROR) { status = DOTNET_PAL_OS_ERROR; done = 1; break; }
            if (header->nlmsg_type != RTM_NEWADDR) continue;
            struct ifaddrmsg *body = NLMSG_DATA(header);
            if (body->ifa_family != AF_INET && body->ifa_family != AF_INET6) continue;
            const uint8_t *local = NULL, *address = NULL; unsigned attributes = (unsigned)IFA_PAYLOAD(header);
            for (struct rtattr *a = IFA_RTA(body); RTA_OK(a, attributes); a = RTA_NEXT(a, attributes)) {
                if (a->rta_type == IFA_LOCAL) local = RTA_DATA(a);
                if (a->rta_type == IFA_ADDRESS) address = RTA_DATA(a);
            }
            if (!local) local = address;
            if (!local || seen++ != index) continue;
            memset(out, 0, sizeof *out);
            out->interface_index = body->ifa_index; out->prefix_length = body->ifa_prefixlen;
            out->address.family = body->ifa_family == AF_INET ? DOTNET_PAL_FAMILY_IPV4 : DOTNET_PAL_FAMILY_IPV6;
            memcpy(out->address.address, local, body->ifa_family == AF_INET ? 4 : 16);
            if (body->ifa_family == AF_INET6 && local[0] == 0xfe && (local[1] & 0xc0) == 0x80) out->address.scope = body->ifa_index;
            status = DOTNET_PAL_OK;
        }
    }
    pthread_mutex_unlock(&dump_lock);
    close(fd);
    return status;
}
static uint32_t deliver(const char *text, uint8_t *out, size_t capacity, size_t *needed) {
    *needed = strlen(text) + 1;
    if (*needed > capacity) return DOTNET_PAL_BUFFER_TOO_SMALL;
    memcpy(out, text, *needed); return DOTNET_PAL_OK;
}
/* A broken answer: the bytes as they are, the length the provider states and its status. */
static uint32_t broken(uint8_t *out, size_t capacity, size_t *needed, const char *bytes, size_t size, size_t stated, uint32_t status) {
    if (size <= capacity) memcpy(out, bytes, size);
    *needed = stated; return status;
}
static uint32_t interface_entry(size_t index, dotnet_pal_network_interface *out, size_t out_size) {
    (void)out_size;
    if (pal_network_fault != 2) return honest_interface(index, out);
    static int step;
    uint32_t status = honest_interface(0, out);
    if (status != DOTNET_PAL_OK) return status;
    switch (step++) {
    case 0: out->index = 0; return DOTNET_PAL_OK;                                  /* the index that means "the target's choice" */
    case 1: memset(out->name, 0, sizeof out->name); return DOTNET_PAL_OK;          /* no name */
    case 2: memset(out->name, 'n', sizeof out->name); return DOTNET_PAL_OK;        /* no terminator */
    case 3: out->kind = 9; return DOTNET_PAL_OK;                                   /* no such kind */
    case 4: out->state = 7; return DOTNET_PAL_OK;                                  /* no such state */
    case 5: out->flags = 0x80; return DOTNET_PAL_OK;                               /* no such flag */
    case 6: out->hardware_address_length = 9; return DOTNET_PAL_OK;                /* longer than the field */
    case 7: return 99u;                                                            /* no such status */
    case 8: return DOTNET_PAL_TIMEOUT;                                             /* a status of the reverse lookup */
    case 9: return DOTNET_PAL_ADDRESS_IN_USE;                                      /* a status of membership */
    default: return DOTNET_PAL_NOT_FOUND;                                          /* outputs written by a failing call */
    }
}
static uint32_t address_entry(size_t index, dotnet_pal_network_address *out, size_t out_size) {
    (void)out_size;
    if (pal_network_fault != 2) return honest_address(index, out);
    static int step;
    memset(out, 0, sizeof *out);
    out->interface_index = 1; out->prefix_length = 8; out->address.family = DOTNET_PAL_FAMILY_IPV4; out->address.address[0] = 127; out->address.address[3] = 1;
    switch (step++) {
    case 0: out->interface_index = 0; return DOTNET_PAL_OK;                        /* an address of no interface */
    case 1: out->address.family = 9; return DOTNET_PAL_OK;                         /* no such family */
    case 2: out->address.port = 80; return DOTNET_PAL_OK;                          /* an endpoint, not an address */
    case 3: out->prefix_length = 33; return DOTNET_PAL_OK;                         /* longer than an IPv4 address */
    case 4: out->address.family = DOTNET_PAL_FAMILY_IPV6; out->prefix_length = 129; return DOTNET_PAL_OK; /* longer than an IPv6 address */
    case 5: return 99u;                                                            /* no such status */
    case 6: return DOTNET_PAL_BUFFER_TOO_SMALL;                                    /* a status of the reverse lookup */
    case 7: out->address.family = DOTNET_PAL_FAMILY_IPV6; out->prefix_length = 128; return DOTNET_PAL_OK; /* the longest prefix is a good one */
    default: return DOTNET_PAL_NOT_FOUND;                                          /* outputs written by a failing call */
    }
}
static uint32_t reverse_lookup(const dotnet_pal_socket_address *address, uint8_t *out, size_t capacity, size_t *needed) {
    if (pal_network_fault == 2) {
        static int step; static char longest[301];
        memset(longest, 'h', sizeof longest - 1);
        switch (step++) {
        case 0: return broken(out, capacity, needed, "hostX", 5, 5, DOTNET_PAL_OK);                           /* no terminator */
        case 1: return broken(out, capacity, needed, "ho\0st", 6, 6, DOTNET_PAL_OK);                           /* terminator inside the text */
        case 2: return broken(out, capacity, needed, longest, sizeof longest, sizeof longest, DOTNET_PAL_OK);  /* longer than any host name */
        case 3: return broken(out, capacity, needed, "host", 5, 257, DOTNET_PAL_BUFFER_TOO_SMALL);             /* needs more than any host name */
        case 4: return broken(out, capacity, needed, "", 1, 1, DOTNET_PAL_OK);                                 /* empty text */
        case 5: return broken(out, capacity, needed, "host", 5, 0, DOTNET_PAL_OK);                             /* no length */
        case 6: return broken(out, capacity, needed, "host", 5, 5, 99u);                                       /* no such status */
        case 7: return broken(out, capacity, needed, "host", 5, 5, DOTNET_PAL_ADDRESS_IN_USE);                 /* a status of membership */
        case 8: return broken(out, capacity, needed, "host", 5, 5, DOTNET_PAL_TIMEOUT);                        /* outputs written by a failing call */
        default: return broken(out, capacity, needed, "host", 5, 5, DOTNET_PAL_NOT_FOUND);
        }
    }
    struct hostent entry, *found = NULL; char strings[4096]; int reason = 0;
    int code = address->family == DOTNET_PAL_FAMILY_IPV4
        ? gethostbyaddr_r(address->address, 4, AF_INET, &entry, strings, sizeof strings, &found, &reason)
        : gethostbyaddr_r(address->address, 16, AF_INET6, &entry, strings, sizeof strings, &found, &reason);
    if (code == 0 && found && found->h_name && found->h_name[0]) return deliver(found->h_name, out, capacity, needed);
    if (code == ERANGE) return DOTNET_PAL_OS_ERROR;
    return reason == TRY_AGAIN ? DOTNET_PAL_TIMEOUT : reason == NO_RECOVERY ? DOTNET_PAL_OS_ERROR : DOTNET_PAL_NOT_FOUND;
}
static uint32_t membership(void *socket, const dotnet_pal_socket_address *group, uint32_t interface_index, uint32_t join) {
    if (pal_network_fault == 2) {
        static int step;
        switch (step++) {
        case 0: return 99u;                           /* no such status */
        case 1: return DOTNET_PAL_BUFFER_TOO_SMALL;   /* a status of the reverse lookup */
        case 2: return DOTNET_PAL_TIMEOUT;
        default: return DOTNET_PAL_WOULD_BLOCK;       /* a status of the sockets group */
        }
    }
    int fd = pal_sockets_host_descriptor(socket), type = 0, domain = 0; socklen_t length = sizeof type;
    if (getsockopt(fd, SOL_SOCKET, SO_TYPE, &type, &length) != 0) return DOTNET_PAL_INVALID_ARGUMENT;
    length = sizeof domain;
    if (getsockopt(fd, SOL_SOCKET, SO_DOMAIN, &domain, &length) != 0) return DOTNET_PAL_INVALID_ARGUMENT;
    int v6 = group->family == DOTNET_PAL_FAMILY_IPV6;
    if (type != SOCK_DGRAM || domain != (v6 ? AF_INET6 : AF_INET)) return DOTNET_PAL_INVALID_ARGUMENT;
    struct group_req request; memset(&request, 0, sizeof request);
    request.gr_interface = interface_index;
    if (v6) {
        struct sockaddr_in6 *to = (struct sockaddr_in6*)&request.gr_group;
        to->sin6_family = AF_INET6; memcpy(&to->sin6_addr, group->address, 16);
    } else {
        struct sockaddr_in *to = (struct sockaddr_in*)&request.gr_group;
        to->sin_family = AF_INET; memcpy(&to->sin_addr, group->address, 4);
    }
    if (setsockopt(fd, v6 ? IPPROTO_IPV6 : IPPROTO_IP, join ? MCAST_JOIN_GROUP : MCAST_LEAVE_GROUP, &request, sizeof request) == 0) return DOTNET_PAL_OK;
    switch (errno) {
    case EADDRINUSE: return DOTNET_PAL_ADDRESS_IN_USE;          /* a member already */
    case EADDRNOTAVAIL: return DOTNET_PAL_ADDRESS_NOT_AVAILABLE; /* not a member */
    case ENODEV: return DOTNET_PAL_NOT_FOUND;                   /* no such interface */
    case EACCES: case EPERM: return DOTNET_PAL_ACCESS_DENIED;
    case ENOPROTOOPT: return DOTNET_PAL_UNSUPPORTED;
    case EINVAL: return DOTNET_PAL_INVALID_ARGUMENT;
    case ENOMEM: case ENOBUFS: return DOTNET_PAL_OUT_OF_MEMORY;
    default: return DOTNET_PAL_OS_ERROR;
    }
}
static const dotnet_pal_host_network table = {
    {DOTNET_PAL_ABI_VERSION, sizeof(dotnet_pal_host_network), DOTNET_PAL_CAP_NETWORK},
    {interface_entry, address_entry, reverse_lookup, membership, NULL},
};
/* Every callback is optional, so no missing callback rejects a table: a header that does not offer the group does. */
static const dotnet_pal_host_network malformed = {
    {DOTNET_PAL_ABI_VERSION, sizeof(dotnet_pal_host_network), 0},
    {interface_entry, address_entry, reverse_lookup, membership, NULL},
};
static const dotnet_pal_host_network silent = {{DOTNET_PAL_ABI_VERSION, sizeof(dotnet_pal_host_network), DOTNET_PAL_CAP_NETWORK}, {NULL, NULL, NULL, NULL, NULL}};
const dotnet_pal_host_network *dotnet_pal_host_network_v2(void) { return pal_network_fault == 1 ? &malformed : pal_network_fault == 3 ? &silent : &table; }

/* Independent POSIX reference provider for the host-packets conformance suite, on
 * the descriptors of the sockets reference provider. It keeps nothing about a
 * socket: the domain and the type are asked of the descriptor each time, and the
 * description of a datagram is whatever control data the kernel attached to it.
 * Fault 1 withholds the callback; fault 2 answers in the ways the front end is
 * documented to sanitize. */
#define _GNU_SOURCE
#include "dotnet_pal.h"
#include <errno.h>
#include <fcntl.h>
#include <netinet/in.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/uio.h>
#include <unistd.h>
int pal_packets_fault;
int pal_sockets_host_descriptor(void *socket);
static int own(int fd, int name) { int value = -1; socklen_t length = sizeof value; return getsockopt(fd, SOL_SOCKET, name, &value, &length) == 0 ? value : -1; }
static uint32_t status_of(int fd, int code) {
    /* With IP_RECVERR the kernel also keeps the error in a queue nothing in the boundary reads, and polls the socket as in error while it is there. */
    struct msghdr queued; memset(&queued, 0, sizeof queued);
    if (code != EAGAIN) while (recvmsg(fd, &queued, MSG_ERRQUEUE | MSG_DONTWAIT) >= 0) {}
    switch (code) {
    /* An expired SO_RCVTIMEO is EAGAIN too; on a blocking descriptor that is the timeout. */
    case EAGAIN: return fcntl(fd, F_GETFL) & O_NONBLOCK ? DOTNET_PAL_WOULD_BLOCK : DOTNET_PAL_TIMEOUT;
    case ECONNREFUSED: return DOTNET_PAL_CONNECTION_REFUSED;
    case ECONNRESET: return DOTNET_PAL_CONNECTION_RESET;
    case ENOTCONN: return DOTNET_PAL_NOT_CONNECTED;
    case EPIPE: return DOTNET_PAL_BROKEN_PIPE;
    case EHOSTUNREACH: return DOTNET_PAL_HOST_UNREACHABLE;
    case ENETUNREACH: case ENETDOWN: return DOTNET_PAL_NETWORK_UNREACHABLE;
    case EMSGSIZE: return DOTNET_PAL_MESSAGE_TOO_LARGE;
    case EACCES: case EPERM: return DOTNET_PAL_ACCESS_DENIED;
    case ETIMEDOUT: return DOTNET_PAL_TIMEOUT;
    case ENOMEM: case ENOBUFS: return DOTNET_PAL_OUT_OF_MEMORY;
    case EINVAL: return DOTNET_PAL_INVALID_ARGUMENT;
    default: return DOTNET_PAL_OS_ERROR;
    }
}
/* An IPv6 address of link scope means something only with the interface it belongs to. */
static uint32_t scope_of(const struct in6_addr *address, uint32_t interface) {
    return IN6_IS_ADDR_LINKLOCAL(address) || IN6_IS_ADDR_MC_LINKLOCAL(address) ? interface : 0;
}
static const dotnet_pal_packet_info with_port = {1, 0, {DOTNET_PAL_FAMILY_IPV4, 9, 0, {127, 0, 0, 1}}}, unknown_family = {1, 0, {9, 0, 0, {127, 0, 0, 1}}},
    dirty = {1, 0xAAAAAAAAu, {DOTNET_PAL_FAMILY_IPV4, 0, 7, {127, 0, 0, 1, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA}}};
static const dotnet_pal_socket_address loopback = {DOTNET_PAL_FAMILY_IPV4, 4242, 0, {127, 0, 0, 1}}, nobody = {9, 4242, 0, {127, 0, 0, 1}};
static uint32_t packet_receive(void *socket, uint8_t *data, size_t capacity, uint32_t flags, dotnet_pal_socket_address *from, dotnet_pal_packet_info *info, size_t info_size, size_t *received) {
    static int step;
    (void)info_size; /* the front end asks with the size of this header's structure */
    if (pal_packets_fault == 2) switch (step++) {
    case 0: *received = capacity + 1; *info = dirty; *from = loopback; return DOTNET_PAL_OK;       /* more than fits */
    case 1: *received = capacity; *info = with_port; *from = loopback; return DOTNET_PAL_OK;       /* a destination is an address, not an endpoint */
    case 2: *received = capacity; *info = unknown_family; *from = nobody; return DOTNET_PAL_OK;    /* families the boundary does not define */
    case 3: *received = capacity; *info = dirty; *from = loopback; return DOTNET_PAL_OK;           /* bytes the address's family does not use, and the reserved field */
    case 4: *received = capacity; *info = dirty; *from = loopback; return DOTNET_PAL_WOULD_BLOCK;  /* outputs written by a failing call */
    case 5: *received = capacity; *info = dirty; *from = loopback; return DOTNET_PAL_NOT_FOUND;    /* a status no receive has */
    default: return 99u;                                                                           /* no such status */
    }
    int fd = pal_sockets_host_descriptor(socket); ssize_t n;
    /* Only a datagram has a destination of its own, and a local socket has no IP address. */
    if (own(fd, SO_DOMAIN) == AF_UNIX || own(fd, SO_TYPE) == SOCK_STREAM) return DOTNET_PAL_INVALID_ARGUMENT;
    struct sockaddr_storage sender; char control[256]; struct iovec part = {data, capacity}; struct msghdr message;
    do {
        memset(&message, 0, sizeof message);
        message.msg_name = &sender; message.msg_namelen = sizeof sender; message.msg_iov = &part; message.msg_iovlen = 1;
        message.msg_control = control; message.msg_controllen = sizeof control;
        n = recvmsg(fd, &message, (flags & DOTNET_PAL_RECEIVE_PEEK) ? MSG_PEEK : 0);
    } while (n < 0 && errno == EINTR);
    if (n < 0) return status_of(fd, errno);
    for (struct cmsghdr *record = CMSG_FIRSTHDR(&message); record; record = CMSG_NXTHDR(&message, record)) {
        if (record->cmsg_level == IPPROTO_IP && record->cmsg_type == IP_PKTINFO) {
            struct in_pktinfo arrived; memcpy(&arrived, CMSG_DATA(record), sizeof arrived);
            /* ipi_addr is the destination in the header; ipi_spec_dst is where a reply would leave from. */
            info->interface_index = (uint32_t)arrived.ipi_ifindex; info->destination.family = DOTNET_PAL_FAMILY_IPV4; memcpy(info->destination.address, &arrived.ipi_addr, 4);
        } else if (record->cmsg_level == IPPROTO_IPV6 && record->cmsg_type == IPV6_PKTINFO) {
            struct in6_pktinfo arrived; memcpy(&arrived, CMSG_DATA(record), sizeof arrived);
            info->interface_index = arrived.ipi6_ifindex; info->destination.family = DOTNET_PAL_FAMILY_IPV6; info->destination.scope = scope_of(&arrived.ipi6_addr, arrived.ipi6_ifindex);
            memcpy(info->destination.address, &arrived.ipi6_addr, 16);
        }
    }
    /* A raw socket's sender has no port, and the kernel reports none. */
    if (!from) { *received = (size_t)n; return DOTNET_PAL_OK; }
    if (sender.ss_family == AF_INET) {
        struct sockaddr_in *v4 = (struct sockaddr_in*)&sender;
        from->family = DOTNET_PAL_FAMILY_IPV4; from->port = ntohs(v4->sin_port); memcpy(from->address, &v4->sin_addr, 4);
    } else if (sender.ss_family == AF_INET6) {
        struct sockaddr_in6 *v6 = (struct sockaddr_in6*)&sender;
        from->family = DOTNET_PAL_FAMILY_IPV6; from->port = ntohs(v6->sin6_port); from->scope = v6->sin6_scope_id; memcpy(from->address, &v6->sin6_addr, 16);
    }
    *received = (size_t)n; return DOTNET_PAL_OK;
}
static const dotnet_pal_host_packets table = {{DOTNET_PAL_ABI_VERSION, sizeof(dotnet_pal_host_packets), DOTNET_PAL_CAP_PACKETS}, {packet_receive, NULL}};
static const dotnet_pal_host_packets malformed = {{DOTNET_PAL_ABI_VERSION, sizeof(dotnet_pal_host_packets), DOTNET_PAL_CAP_PACKETS}, {NULL, NULL}};
const dotnet_pal_host_packets *dotnet_pal_host_packets_v2(void) { return pal_packets_fault == 1 ? &malformed : &table; }

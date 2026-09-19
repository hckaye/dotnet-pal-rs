/* System.Native over the boundary: sockets, readiness events and name resolution.
 *
 * The boundary's sockets group speaks its own address layout and status codes;
 * this unit gives managed code the System.Native face of it. Managed code never
 * looks inside a socket address: it asks this library for the sizes and reads or
 * writes every field through the accessors below, so the "sockaddr" it carries
 * around is simply the boundary's dotnet_pal_socket_address.
 *
 * The socket engine of the BCL expects edge-triggered readiness (epoll with
 * EPOLLET, kqueue with EV_CLEAR): one event when a socket becomes ready, and
 * another only after an operation has reported that it would block. The
 * boundary has a level-triggered poll and a wake. An event port here keeps, per
 * registered socket, which events it has delivered; it polls only for the ones
 * not delivered, an operation that would block re-arms its direction and wakes
 * the waiter, and so does every change of the registrations.
 *
 * Not carried by the boundary, and reported as ENOTSUP/EAFNOSUPPORT: Unix domain
 * and raw sockets, out-of-band data, control messages and packet information,
 * multicast, TCP keep-alive tuning, reverse name lookup and interface names. */
#include "system_native_internal.h"

static int32_t net_error(uint32_t status) { return SystemNative_ConvertErrorPlatformToPal(sn_errno(status)); }
static int32_t errno_error(void) { return SystemNative_ConvertErrorPlatformToPal(errno); }
/* Pins a socket descriptor; on failure *error holds the answer for the caller to return. */
static sn_object *socket_of(intptr_t fd, const dotnet_pal_sockets_ops **ops, int32_t *error) {
    *ops = sn_sockets();
    sn_object *object = sn_pin(fd, SN_SOCKET, ENOTSOCK);
    if (!object) { *error = errno_error(); return NULL; }
    if (!*ops) { sn_unpin(object); *error = Error_ENOTSUP; return NULL; }
    return object;
}

/* ---- socket addresses ----------------------------------------------------------------- */
#define ADDRESS_SIZE ((int32_t)sizeof(dotnet_pal_socket_address))
static int32_t family_to_pal(uint16_t family) { return family == DOTNET_PAL_FAMILY_IPV4 ? AddressFamily_AF_INET : family == DOTNET_PAL_FAMILY_IPV6 ? AddressFamily_AF_INET6 : AddressFamily_AF_UNSPEC; }
/* Managed buffers carry no alignment promise: addresses move through memcpy. */
static bool load_address(const uint8_t *buffer, int32_t length, dotnet_pal_socket_address *out) {
    if (!buffer || length < ADDRESS_SIZE) return false;
    memcpy(out, buffer, sizeof *out);
    return out->family == DOTNET_PAL_FAMILY_IPV4 || out->family == DOTNET_PAL_FAMILY_IPV6;
}
static void store_address(uint8_t *buffer, int32_t *length, const dotnet_pal_socket_address *address) {
    if (!buffer || !length || *length < ADDRESS_SIZE) { if (length) *length = 0; return; }
    memcpy(buffer, address, sizeof *address);
    *length = address->family == 0 ? 0 : ADDRESS_SIZE;
}
PALEXPORT int32_t SystemNative_GetSocketAddressSizes(int32_t* ipv4SocketAddressSize, int32_t* ipv6SocketAddressSize, int32_t* udsSocketAddressSize, int32_t* maxSocketAddressSize) {
    if (!ipv4SocketAddressSize || !ipv6SocketAddressSize || !udsSocketAddressSize || !maxSocketAddressSize) return Error_EFAULT;
    *ipv4SocketAddressSize = ADDRESS_SIZE; *ipv6SocketAddressSize = ADDRESS_SIZE; *udsSocketAddressSize = 110; *maxSocketAddressSize = 128;
    return Error_SUCCESS;
}
PALEXPORT int32_t SystemNative_GetMaximumAddressSize(void) { return 128; }
PALEXPORT void SystemNative_GetDomainSocketSizes(int32_t* pathOffset, int32_t* pathSize, int32_t* addressSize) { *pathOffset = 2; *pathSize = 108; *addressSize = 110; }
PALEXPORT int32_t SystemNative_GetAddressFamily(const uint8_t* socketAddress, int32_t socketAddressLen, int32_t* addressFamily) {
    uint16_t family;
    if (!socketAddress || !addressFamily || socketAddressLen < (int32_t)sizeof family) return Error_EFAULT;
    memcpy(&family, socketAddress, sizeof family);
    *addressFamily = family_to_pal(family);
    return Error_SUCCESS;
}
PALEXPORT int32_t SystemNative_SetAddressFamily(uint8_t* socketAddress, int32_t socketAddressLen, int32_t addressFamily) {
    uint16_t family = addressFamily == AddressFamily_AF_INET ? DOTNET_PAL_FAMILY_IPV4 : addressFamily == AddressFamily_AF_INET6 ? DOTNET_PAL_FAMILY_IPV6 : 0;
    if (!socketAddress || socketAddressLen < (int32_t)sizeof family) return Error_EFAULT;
    if (family == 0 && addressFamily != AddressFamily_AF_UNSPEC) return Error_EAFNOSUPPORT;
    memcpy(socketAddress, &family, sizeof family);
    return Error_SUCCESS;
}
PALEXPORT int32_t SystemNative_GetPort(const uint8_t* socketAddress, int32_t socketAddressLen, uint16_t* port) {
    dotnet_pal_socket_address address;
    if (!port) return Error_EFAULT;
    if (!load_address(socketAddress, socketAddressLen, &address)) return Error_EAFNOSUPPORT;
    *port = address.port;
    return Error_SUCCESS;
}
PALEXPORT int32_t SystemNative_SetPort(uint8_t* socketAddress, int32_t socketAddressLen, uint16_t port) {
    dotnet_pal_socket_address address;
    if (!load_address(socketAddress, socketAddressLen, &address)) return Error_EAFNOSUPPORT;
    address.port = port;
    memcpy(socketAddress, &address, sizeof address);
    return Error_SUCCESS;
}
/* The 32-bit value is the four address bytes in memory order, as it is in sockaddr_in. */
PALEXPORT int32_t SystemNative_GetIPv4Address(const uint8_t* socketAddress, int32_t socketAddressLen, uint32_t* address) {
    dotnet_pal_socket_address value;
    if (!address) return Error_EFAULT;
    if (!load_address(socketAddress, socketAddressLen, &value) || value.family != DOTNET_PAL_FAMILY_IPV4) return Error_EAFNOSUPPORT;
    memcpy(address, value.address, 4);
    return Error_SUCCESS;
}
PALEXPORT int32_t SystemNative_SetIPv4Address(uint8_t* socketAddress, int32_t socketAddressLen, uint32_t address) {
    dotnet_pal_socket_address value;
    if (!load_address(socketAddress, socketAddressLen, &value) || value.family != DOTNET_PAL_FAMILY_IPV4) return Error_EAFNOSUPPORT;
    memset(value.address, 0, sizeof value.address); memcpy(value.address, &address, 4); value.scope = 0;
    memcpy(socketAddress, &value, sizeof value);
    return Error_SUCCESS;
}
PALEXPORT int32_t SystemNative_GetIPv6Address(const uint8_t* socketAddress, int32_t socketAddressLen, uint8_t* address, int32_t addressLen, uint32_t* scopeId) {
    dotnet_pal_socket_address value;
    if (!address || !scopeId || addressLen < 16) return Error_EFAULT;
    if (!load_address(socketAddress, socketAddressLen, &value) || value.family != DOTNET_PAL_FAMILY_IPV6) return Error_EAFNOSUPPORT;
    memcpy(address, value.address, 16); *scopeId = value.scope;
    return Error_SUCCESS;
}
PALEXPORT int32_t SystemNative_SetIPv6Address(uint8_t* socketAddress, int32_t socketAddressLen, uint8_t* address, int32_t addressLen, uint32_t scopeId) {
    dotnet_pal_socket_address value;
    if (!address || addressLen < 16) return Error_EFAULT;
    if (!load_address(socketAddress, socketAddressLen, &value) || value.family != DOTNET_PAL_FAMILY_IPV6) return Error_EAFNOSUPPORT;
    memcpy(value.address, address, 16); value.scope = scopeId;
    memcpy(socketAddress, &value, sizeof value);
    return Error_SUCCESS;
}

/* ---- creation and connection ---------------------------------------------------------------- */
static intptr_t adopt(void *handle, int32_t family, int32_t type, int32_t protocol) {
    sn_object *object = sn_new(SN_SOCKET, handle);
    if (!object) { const dotnet_pal_sockets_ops *s = sn_sockets(); if (s) (void)s->close(handle); return -1; }
    object->family = family; object->type = type; object->protocol = protocol;
    return sn_install(object);
}
PALEXPORT int32_t SystemNative_Socket(int32_t addressFamily, int32_t socketType, int32_t protocolType, intptr_t* createdSocket) {
    const dotnet_pal_sockets_ops *s = sn_sockets(); void *handle = NULL;
    if (!createdSocket) return Error_EFAULT;
    *createdSocket = -1;
    uint32_t family = addressFamily == AddressFamily_AF_INET ? DOTNET_PAL_FAMILY_IPV4 : addressFamily == AddressFamily_AF_INET6 ? DOTNET_PAL_FAMILY_IPV6 : 0;
    if (!s || family == 0) return Error_EAFNOSUPPORT; /* without the group no address family exists */
    uint32_t kind = socketType == SocketType_SOCK_STREAM ? DOTNET_PAL_SOCKET_STREAM : socketType == SocketType_SOCK_DGRAM ? DOTNET_PAL_SOCKET_DATAGRAM : 0;
    if (kind == 0) return Error_EPROTOTYPE;
    int32_t natural = kind == DOTNET_PAL_SOCKET_STREAM ? ProtocolType_PT_TCP : ProtocolType_PT_UDP;
    if (protocolType != ProtocolType_PT_UNSPECIFIED && protocolType != natural) return Error_EPROTONOSUPPORT;
    uint32_t status = s->create(family, kind, &handle);
    if (status != DOTNET_PAL_OK) return status == DOTNET_PAL_UNSUPPORTED ? Error_EAFNOSUPPORT : net_error(status);
    intptr_t fd = adopt(handle, addressFamily, socketType, natural);
    if (fd < 0) return errno_error();
    *createdSocket = fd;
    return Error_SUCCESS;
}
PALEXPORT int32_t SystemNative_Bind(intptr_t socket, int32_t protocolType, uint8_t* socketAddress, int32_t socketAddressLen) {
    const dotnet_pal_sockets_ops *s; int32_t error; dotnet_pal_socket_address address; (void)protocolType;
    if (!load_address(socketAddress, socketAddressLen, &address)) return Error_EAFNOSUPPORT;
    sn_object *object = socket_of(socket, &s, &error);
    if (!object) return error;
    error = net_error(s->bind(object->handle, &address));
    sn_unpin(object);
    return error;
}
PALEXPORT int32_t SystemNative_Listen(intptr_t socket, int32_t backlog) {
    const dotnet_pal_sockets_ops *s; int32_t error;
    sn_object *object = socket_of(socket, &s, &error);
    if (!object) return error;
    error = net_error(s->listen(object->handle, backlog < 0 ? 0 : (uint32_t)backlog));
    if (error == Error_SUCCESS) object->listening = true;
    sn_unpin(object);
    return error;
}
PALEXPORT int32_t SystemNative_Accept(intptr_t socket, uint8_t* socketAddress, int32_t* socketAddressLen, intptr_t* acceptedSocket) {
    const dotnet_pal_sockets_ops *s; int32_t error; void *handle = NULL; dotnet_pal_socket_address peer;
    if (!acceptedSocket) return Error_EFAULT;
    *acceptedSocket = -1;
    sn_object *object = socket_of(socket, &s, &error);
    if (!object) return error;
    uint32_t status = s->accept(object->handle, &handle, &peer);
    if (status == DOTNET_PAL_WOULD_BLOCK) sn_socket_would_block(object, SocketEvents_SA_READ);
    if (status == DOTNET_PAL_OK) {
        intptr_t fd = adopt(handle, object->family, object->type, object->protocol);
        if (fd < 0) error = errno_error(); else { *acceptedSocket = fd; store_address(socketAddress, socketAddressLen, &peer); error = Error_SUCCESS; }
    } else error = net_error(status);
    sn_unpin(object);
    return error;
}
PALEXPORT int32_t SystemNative_Connect(intptr_t socket, uint8_t* socketAddress, int32_t socketAddressLen) {
    const dotnet_pal_sockets_ops *s; int32_t error; dotnet_pal_socket_address address;
    if (!load_address(socketAddress, socketAddressLen, &address)) return Error_EAFNOSUPPORT;
    sn_object *object = socket_of(socket, &s, &error);
    if (!object) return error;
    uint32_t status = s->connect(object->handle, &address);
    /* Completion of a connection in progress shows as writability. */
    if (status == DOTNET_PAL_IN_PROGRESS) sn_socket_would_block(object, SocketEvents_SA_WRITE);
    sn_unpin(object);
    return net_error(status);
}
/* Data with the connection request is an optimization the boundary does not have: connect, and send nothing. */
PALEXPORT int32_t SystemNative_Connectx(intptr_t socket, uint8_t* socketAddress, int32_t socketAddressLen, uint8_t* data, int32_t dataLen, int32_t tfo, int* sent) {
    (void)data; (void)dataLen; (void)tfo;
    if (sent) *sent = 0;
    return SystemNative_Connect(socket, socketAddress, socketAddressLen);
}
PALEXPORT int32_t SystemNative_Shutdown(intptr_t socket, int32_t socketShutdown) {
    const dotnet_pal_sockets_ops *s; int32_t error;
    uint32_t how = socketShutdown == SocketShutdown_SHUT_READ ? DOTNET_PAL_SHUTDOWN_READ : socketShutdown == SocketShutdown_SHUT_WRITE ? DOTNET_PAL_SHUTDOWN_WRITE
        : socketShutdown == SocketShutdown_SHUT_BOTH ? DOTNET_PAL_SHUTDOWN_BOTH : 0;
    if (how == 0) return Error_EINVAL;
    sn_object *object = socket_of(socket, &s, &error);
    if (!object) return error;
    error = net_error(s->shutdown(object->handle, how));
    sn_unpin(object);
    return error;
}
/* The BCL disconnects a stream to release threads blocked on it before closing. Shutting both directions
 * down does that; the abortive part of its close travels separately, as the linger option it sets next. */
PALEXPORT int32_t SystemNative_Disconnect(intptr_t socket) {
    int32_t error = SystemNative_Shutdown(socket, SocketShutdown_SHUT_BOTH);
    return error == Error_ENOTCONN ? Error_SUCCESS : error;
}
static int32_t endpoint(intptr_t socket, uint8_t* socketAddress, int32_t* socketAddressLen, bool peer) {
    const dotnet_pal_sockets_ops *s; int32_t error; dotnet_pal_socket_address address;
    if (!socketAddress || !socketAddressLen || *socketAddressLen < ADDRESS_SIZE) return Error_EFAULT;
    sn_object *object = socket_of(socket, &s, &error);
    if (!object) return error;
    uint32_t status = (peer ? s->peer_address : s->local_address)(object->handle, &address);
    if (status == DOTNET_PAL_OK) store_address(socketAddress, socketAddressLen, &address);
    sn_unpin(object);
    return net_error(status);
}
PALEXPORT int32_t SystemNative_GetPeerName(intptr_t socket, uint8_t* socketAddress, int32_t* socketAddressLen) { return endpoint(socket, socketAddress, socketAddressLen, true); }
PALEXPORT int32_t SystemNative_GetSockName(intptr_t socket, uint8_t* socketAddress, int32_t* socketAddressLen) { return endpoint(socket, socketAddress, socketAddressLen, false); }
PALEXPORT int32_t SystemNative_GetSocketType(intptr_t socket, int32_t* addressFamily, int32_t* socketType, int32_t* protocolType, int32_t* isListening) {
    if (!addressFamily || !socketType || !protocolType || !isListening) return Error_EFAULT;
    sn_object *object = sn_pin(socket, SN_SOCKET, ENOTSOCK);
    if (!object) return errno_error();
    *addressFamily = object->family; *socketType = object->type; *protocolType = object->protocol; *isListening = object->listening;
    sn_unpin(object);
    return Error_SUCCESS;
}
PALEXPORT int32_t SystemNative_GetPeerID(intptr_t socket, uint32_t* euid) { (void)socket; (void)euid; return sn_fail(ENOTSUP); }

/* ---- transfers ---------------------------------------------------------------------------------- */
static bool receive_flags(int32_t flags, uint32_t *out) {
    if (flags & ~(SocketFlags_MSG_PEEK)) return false;
    *out = (flags & SocketFlags_MSG_PEEK) ? DOTNET_PAL_RECEIVE_PEEK : 0;
    return true;
}
static int32_t transfer_result(sn_object *object, uint32_t status, int32_t events) {
    if (status == DOTNET_PAL_WOULD_BLOCK) sn_socket_would_block(object, events);
    return net_error(status);
}
PALEXPORT int32_t SystemNative_Receive(intptr_t socket, void* buffer, int32_t bufferLen, int32_t flags, int32_t* received) {
    const dotnet_pal_sockets_ops *s; int32_t error; uint32_t wanted; size_t got = 0;
    if (!buffer || bufferLen < 0 || !received) return Error_EFAULT;
    *received = 0;
    if (!receive_flags(flags, &wanted)) return Error_ENOTSUP;
    sn_object *object = socket_of(socket, &s, &error);
    if (!object) return error;
    error = transfer_result(object, s->receive(object->handle, buffer, (size_t)bufferLen, wanted, NULL, &got), SocketEvents_SA_READ);
    if (error == Error_SUCCESS) *received = (int32_t)got;
    sn_unpin(object);
    return error;
}
PALEXPORT int32_t SystemNative_Send(intptr_t socket, void* buffer, int32_t bufferLen, int32_t flags, int32_t* sent) {
    const dotnet_pal_sockets_ops *s; int32_t error; size_t done = 0;
    if (!buffer || bufferLen < 0 || !sent) return Error_EFAULT;
    *sent = 0;
    if (flags != 0) return Error_ENOTSUP;
    sn_object *object = socket_of(socket, &s, &error);
    if (!object) return error;
    error = transfer_result(object, s->send(object->handle, buffer, (size_t)bufferLen, NULL, &done), SocketEvents_SA_WRITE);
    if (error == Error_SUCCESS) *sent = (int32_t)done;
    sn_unpin(object);
    return error;
}
static bool total_length(const MessageHeader *header, size_t *total) {
    *total = 0;
    if (header->IOVectorCount < 0 || (header->IOVectorCount > 0 && !header->IOVectors)) return false;
    for (int32_t i = 0; i < header->IOVectorCount; ++i) if (__builtin_add_overflow(*total, header->IOVectors[i].Count, total)) return false;
    return true;
}
/* A message is one send: several buffers of a datagram are gathered first, because the boundary sends
 * one buffer at a time and two sends would be two datagrams. A stream may stop after any buffer. */
PALEXPORT int32_t SystemNative_SendMessage(intptr_t socket, MessageHeader* messageHeader, int32_t flags, int64_t* sent) {
    const dotnet_pal_sockets_ops *s; int32_t error; size_t total, done = 0; dotnet_pal_socket_address to; bool addressed;
    if (!messageHeader || !sent || messageHeader->SocketAddressLen < 0 || messageHeader->ControlBufferLen < 0) return Error_EFAULT;
    *sent = 0;
    if (flags != 0) return Error_ENOTSUP;
    if (!total_length(messageHeader, &total)) return Error_EFAULT;
    addressed = messageHeader->SocketAddress && messageHeader->SocketAddressLen > 0;
    if (addressed && !load_address(messageHeader->SocketAddress, messageHeader->SocketAddressLen, &to)) return Error_EAFNOSUPPORT;
    sn_object *object = socket_of(socket, &s, &error);
    if (!object) return error;
    uint32_t status = DOTNET_PAL_OK;
    if (messageHeader->IOVectorCount == 1 || total == 0) {
        const uint8_t *only = messageHeader->IOVectorCount > 0 ? messageHeader->IOVectors[0].Base : NULL;
        status = s->send(object->handle, only, total, addressed ? &to : NULL, &done);
    } else if (object->type == SocketType_SOCK_DGRAM) {
        uint8_t *gathered = SystemNative_Malloc(total);
        if (!gathered) status = DOTNET_PAL_OUT_OF_MEMORY;
        else {
            size_t at = 0;
            for (int32_t i = 0; i < messageHeader->IOVectorCount; ++i) { memcpy(gathered + at, messageHeader->IOVectors[i].Base, messageHeader->IOVectors[i].Count); at += messageHeader->IOVectors[i].Count; }
            status = s->send(object->handle, gathered, total, addressed ? &to : NULL, &done);
            SystemNative_Free(gathered);
        }
    } else {
        for (int32_t i = 0; i < messageHeader->IOVectorCount && status == DOTNET_PAL_OK; ++i) {
            size_t part = 0, count = messageHeader->IOVectors[i].Count;
            if (count == 0) continue;
            status = s->send(object->handle, messageHeader->IOVectors[i].Base, count, NULL, &part);
            done += part;
            if (part < count) break;
        }
        /* Bytes already accepted are the result; the condition that stopped the rest shows on the next call. */
        if (done > 0 && status != DOTNET_PAL_OK) { if (status == DOTNET_PAL_WOULD_BLOCK) sn_socket_would_block(object, SocketEvents_SA_WRITE); status = DOTNET_PAL_OK; }
    }
    error = transfer_result(object, status, SocketEvents_SA_WRITE);
    if (error == Error_SUCCESS) *sent = (int64_t)done;
    sn_unpin(object);
    return error;
}
/* One receive into the first buffer of a stream (a short read is a valid read); a datagram is received
 * whole and scattered, since a second receive would be the next datagram. No control data exists. */
PALEXPORT int32_t SystemNative_ReceiveMessage(intptr_t socket, MessageHeader* messageHeader, int32_t flags, int64_t* received) {
    const dotnet_pal_sockets_ops *s; int32_t error; uint32_t wanted; size_t total, got = 0; dotnet_pal_socket_address from;
    if (!messageHeader || !received || messageHeader->SocketAddressLen < 0 || messageHeader->ControlBufferLen < 0) return Error_EFAULT;
    *received = 0;
    if (!receive_flags(flags, &wanted)) return Error_ENOTSUP;
    if (!total_length(messageHeader, &total)) return Error_EFAULT;
    sn_object *object = socket_of(socket, &s, &error);
    if (!object) return error;
    int32_t first = 0;
    while (first < messageHeader->IOVectorCount && messageHeader->IOVectors[first].Count == 0) first++;
    uint32_t status;
    memset(&from, 0, sizeof from);
    if (first >= messageHeader->IOVectorCount) status = s->receive(object->handle, NULL, 0, wanted, &from, &got);
    else if (object->type != SocketType_SOCK_DGRAM || first == messageHeader->IOVectorCount - 1)
        status = s->receive(object->handle, messageHeader->IOVectors[first].Base, messageHeader->IOVectors[first].Count, wanted, &from, &got);
    else {
        uint8_t *whole = SystemNative_Malloc(total);
        if (!whole) status = DOTNET_PAL_OUT_OF_MEMORY;
        else {
            status = s->receive(object->handle, whole, total, wanted, &from, &got);
            size_t at = 0;
            for (int32_t i = first; status == DOTNET_PAL_OK && i < messageHeader->IOVectorCount && at < got; ++i) {
                size_t part = messageHeader->IOVectors[i].Count < got - at ? messageHeader->IOVectors[i].Count : got - at;
                memcpy(messageHeader->IOVectors[i].Base, whole + at, part); at += part;
            }
            SystemNative_Free(whole);
        }
    }
    error = transfer_result(object, status, SocketEvents_SA_READ);
    if (error == Error_SUCCESS) {
        *received = (int64_t)got;
        if (messageHeader->SocketAddress) store_address(messageHeader->SocketAddress, &messageHeader->SocketAddressLen, &from);
        messageHeader->ControlBufferLen = 0; messageHeader->Flags = 0;
    }
    sn_unpin(object);
    return error;
}
PALEXPORT int32_t SystemNative_ReceiveSocketError(intptr_t socket, MessageHeader* messageHeader) { (void)socket; (void)messageHeader; return Error_ENOTSUP; }
PALEXPORT int32_t SystemNative_SendFile(intptr_t out_fd, intptr_t in_fd, int64_t offset, int64_t count, int64_t* sent) {
    if (!sent || offset < 0 || count < 0) return Error_EFAULT;
    *sent = 0;
    uint8_t *buffer = SystemNative_Malloc(64 * 1024);
    if (!buffer) return Error_ENOMEM;
    int32_t error = Error_SUCCESS;
    while (*sent < count && error == Error_SUCCESS) {
        int64_t left = count - *sent;
        int32_t got = SystemNative_PRead(in_fd, buffer, left > 64 * 1024 ? 64 * 1024 : (int32_t)left, offset + *sent);
        if (got < 0) { error = errno_error(); break; }
        if (got == 0) break;
        int32_t put = 0;
        error = SystemNative_Send(out_fd, buffer, got, 0, &put);
        *sent += put;
        if (put < got) break;
    }
    SystemNative_Free(buffer);
    /* Progress is the result, exactly as sendfile(2) reports a partial transfer. */
    return *sent > 0 ? Error_SUCCESS : error;
}

/* ---- options ---------------------------------------------------------------------------------------- */
static uint32_t option_of(int32_t level, int32_t name, bool *inverted) {
    *inverted = false;
    if (level == SocketOptionLevel_SOL_SOCKET) switch (name) {
        case SocketOptionName_SO_REUSEADDR: return DOTNET_PAL_SOCKET_REUSE_ADDRESS;
        /* Exclusive use is the inverse of address reuse, as it is in the reference implementation. */
        case SocketOptionName_SO_EXCLUSIVEADDRUSE: *inverted = true; return DOTNET_PAL_SOCKET_REUSE_ADDRESS;
        case SocketOptionName_SO_KEEPALIVE: return DOTNET_PAL_SOCKET_KEEP_ALIVE;
        case SocketOptionName_SO_BROADCAST: return DOTNET_PAL_SOCKET_BROADCAST;
        case SocketOptionName_SO_SNDBUF: return DOTNET_PAL_SOCKET_SEND_BUFFER;
        case SocketOptionName_SO_RCVBUF: return DOTNET_PAL_SOCKET_RECEIVE_BUFFER;
        case SocketOptionName_SO_SNDTIMEO: return DOTNET_PAL_SOCKET_SEND_TIMEOUT;
        case SocketOptionName_SO_RCVTIMEO: return DOTNET_PAL_SOCKET_RECEIVE_TIMEOUT;
        default: return 0;
    }
    if (level == SocketOptionLevel_SOL_TCP) switch (name) {
        case SocketOptionName_SO_TCP_NODELAY: return DOTNET_PAL_SOCKET_NO_DELAY;
        case SocketOptionName_SO_TCP_KEEPALIVE_TIME: return DOTNET_PAL_SOCKET_KEEP_ALIVE_IDLE;
        case SocketOptionName_SO_TCP_KEEPALIVE_INTERVAL: return DOTNET_PAL_SOCKET_KEEP_ALIVE_INTERVAL;
        case SocketOptionName_SO_TCP_KEEPALIVE_RETRYCOUNT: return DOTNET_PAL_SOCKET_KEEP_ALIVE_COUNT;
        default: return 0;
    }
    /* The same names serve both IP levels; the provider applies them to the family of the socket. */
    if (level == SocketOptionLevel_SOL_IP || level == SocketOptionLevel_SOL_IPV6) switch (name) {
        case SocketOptionName_SO_IP_TTL: case SocketOptionName_SO_IPV6_HOPLIMIT: return DOTNET_PAL_SOCKET_HOPS;
        case SocketOptionName_SO_IP_MULTICAST_TTL: return DOTNET_PAL_SOCKET_MULTICAST_HOPS;
        case SocketOptionName_SO_IP_MULTICAST_LOOP: *inverted = false; return DOTNET_PAL_SOCKET_MULTICAST_LOOPBACK;
        case SocketOptionName_SO_IP_MULTICAST_IF: return level == SocketOptionLevel_SOL_IPV6 ? DOTNET_PAL_SOCKET_MULTICAST_INTERFACE : 0; /* IPv4 names it by address; see SetIPv4MulticastOption */
        case SocketOptionName_SO_IPV6_V6ONLY: return level == SocketOptionLevel_SOL_IPV6 ? DOTNET_PAL_SOCKET_IPV6_ONLY : 0;
        default: return 0;
    }
    return 0;
}
static bool is_flag(uint32_t option) {
    return option == DOTNET_PAL_SOCKET_REUSE_ADDRESS || option == DOTNET_PAL_SOCKET_NO_DELAY || option == DOTNET_PAL_SOCKET_KEEP_ALIVE
        || option == DOTNET_PAL_SOCKET_BROADCAST || option == DOTNET_PAL_SOCKET_IPV6_ONLY || option == DOTNET_PAL_SOCKET_MULTICAST_LOOPBACK;
}
PALEXPORT int32_t SystemNative_GetSockOpt(intptr_t socket, int32_t socketOptionLevel, int32_t socketOptionName, uint8_t* optionValue, int32_t* optionLen) {
    const dotnet_pal_sockets_ops *s; int32_t error, result; bool inverted; uint64_t value = 0;
    if (!optionValue || !optionLen || *optionLen < (int32_t)sizeof(int32_t)) return Error_EFAULT;
    sn_object *object = socket_of(socket, &s, &error);
    if (!object) return error;
    if (socketOptionLevel == SocketOptionLevel_SOL_SOCKET && socketOptionName == SocketOptionName_SO_TYPE) { result = object->type; error = Error_SUCCESS; }
    else if (socketOptionLevel == SocketOptionLevel_SOL_SOCKET && socketOptionName == SocketOptionName_SO_ERROR) {
        /* This option reports the platform error number; GetSocketErrorOption is the converted form. */
        error = net_error(s->get_option(object->handle, DOTNET_PAL_SOCKET_ERROR, &value));
        result = sn_errno((uint32_t)value);
    } else {
        uint32_t option = option_of(socketOptionLevel, socketOptionName, &inverted);
        if (option == 0) { sn_unpin(object); return Error_ENOTSUP; }
        error = net_error(s->get_option(object->handle, option, &value));
        result = is_flag(option) ? ((value != 0) != inverted) : value > INT32_MAX ? INT32_MAX : (int32_t)value;
    }
    sn_unpin(object);
    if (error != Error_SUCCESS) return error;
    memcpy(optionValue, &result, sizeof result); *optionLen = (int32_t)sizeof result;
    return Error_SUCCESS;
}
PALEXPORT int32_t SystemNative_SetSockOpt(intptr_t socket, int32_t socketOptionLevel, int32_t socketOptionName, uint8_t* optionValue, int32_t optionLen) {
    const dotnet_pal_sockets_ops *s; int32_t error, value; bool inverted;
    if (!optionValue || optionLen < (int32_t)sizeof value) return Error_EFAULT;
    memcpy(&value, optionValue, sizeof value);
    uint32_t option = option_of(socketOptionLevel, socketOptionName, &inverted);
    if (option == 0) return Error_ENOTSUP;
    if (value < 0) return Error_EINVAL;
    sn_object *object = socket_of(socket, &s, &error);
    if (!object) return error;
    error = net_error(s->set_option(object->handle, option, is_flag(option) ? (uint64_t)((value != 0) != inverted) : (uint64_t)value));
    sn_unpin(object);
    return error;
}
PALEXPORT int32_t SystemNative_GetRawSockOpt(intptr_t socket, int32_t socketOptionLevel, int32_t socketOptionName, uint8_t* optionValue, int32_t* optionLen) {
    (void)socket; (void)socketOptionLevel; (void)socketOptionName; (void)optionValue; (void)optionLen; return Error_ENOTSUP;
}
PALEXPORT int32_t SystemNative_SetRawSockOpt(intptr_t socket, int32_t socketOptionLevel, int32_t socketOptionName, uint8_t* optionValue, int32_t optionLen) {
    (void)socket; (void)socketOptionLevel; (void)socketOptionName; (void)optionValue; (void)optionLen; return Error_ENOTSUP;
}
static int32_t numeric_option(intptr_t socket, uint32_t option, uint64_t *value, bool set) {
    const dotnet_pal_sockets_ops *s; int32_t error;
    sn_object *object = socket_of(socket, &s, &error);
    if (!object) return error;
    error = net_error(set ? s->set_option(object->handle, option, *value) : s->get_option(object->handle, option, value));
    sn_unpin(object);
    return error;
}
PALEXPORT int32_t SystemNative_GetSocketErrorOption(intptr_t socket, int32_t* error) {
    uint64_t value = 0;
    if (!error) return Error_EFAULT;
    int32_t result = numeric_option(socket, DOTNET_PAL_SOCKET_ERROR, &value, false);
    if (result == Error_SUCCESS) *error = net_error((uint32_t)value);
    return result;
}
PALEXPORT int32_t SystemNative_GetLingerOption(intptr_t socket, LingerOption* option) {
    uint64_t value = 0;
    if (!option) return Error_EFAULT;
    int32_t result = numeric_option(socket, DOTNET_PAL_SOCKET_LINGER, &value, false);
    if (result == Error_SUCCESS) { option->OnOff = value != 0; option->Seconds = value == 0 ? 0 : (int32_t)(value - 1); }
    return result;
}
PALEXPORT int32_t SystemNative_SetLingerOption(intptr_t socket, LingerOption* option) {
    if (!option) return Error_EFAULT;
    if (option->OnOff != 0 && (option->Seconds < 0 || option->Seconds > UINT16_MAX)) return Error_EINVAL;
    uint64_t value = option->OnOff == 0 ? 0 : (uint64_t)option->Seconds + 1;
    return numeric_option(socket, DOTNET_PAL_SOCKET_LINGER, &value, true);
}
PALEXPORT int32_t SystemNative_SetReceiveTimeout(intptr_t socket, int32_t millisecondsTimeout) {
    uint64_t value = (uint64_t)millisecondsTimeout;
    return millisecondsTimeout < 0 ? Error_EINVAL : numeric_option(socket, DOTNET_PAL_SOCKET_RECEIVE_TIMEOUT, &value, true);
}
PALEXPORT int32_t SystemNative_SetSendTimeout(intptr_t socket, int32_t millisecondsTimeout) {
    uint64_t value = (uint64_t)millisecondsTimeout;
    return millisecondsTimeout < 0 ? Error_EINVAL : numeric_option(socket, DOTNET_PAL_SOCKET_SEND_TIMEOUT, &value, true);
}
PALEXPORT int32_t SystemNative_GetBytesAvailable(intptr_t socket, int32_t* available) {
    uint64_t value = 0;
    if (!available) return Error_EFAULT;
    int32_t result = numeric_option(socket, DOTNET_PAL_SOCKET_AVAILABLE, &value, false);
    *available = result == Error_SUCCESS ? (value > INT32_MAX ? INT32_MAX : (int32_t)value) : 0;
    return result;
}
PALEXPORT int32_t SystemNative_GetAtOutOfBandMark(intptr_t socket, int32_t* available) { (void)socket; if (available) *available = 0; return Error_ENOTSUP; }
PALEXPORT int32_t SystemNative_GetControlMessageBufferSize(int32_t isIPv4, int32_t isIPv6) { (void)isIPv4; (void)isIPv6; return 0; }
PALEXPORT int32_t SystemNative_TryGetIPPacketInformation(MessageHeader* messageHeader, int32_t isIPv4, IPPacketInformation* packetInfo) { (void)messageHeader; (void)isIPv4; (void)packetInfo; return 0; }
PALEXPORT int32_t SystemNative_PlatformSupportsDualModeIPv4PacketInfo(void) { return 0; }

/* ---- multicast ------------------------------------------------------------------------------------------ */
/* A membership names its interface by index. The BCL may name it by one of its addresses instead
 * (UdpClient.JoinMulticastGroup(group, localAddress)); the address list of the network group translates. */
static bool interface_of_address(const dotnet_pal_network_ops *n, const uint8_t *address, size_t size, uint32_t *index) {
    dotnet_pal_network_address entry;
    for (size_t i = 0; n->address_entry && i < 65536; ++i) {
        if (n->address_entry(i, &entry, sizeof entry) != DOTNET_PAL_OK) break;
        if (entry.address.family == (size == 4 ? DOTNET_PAL_FAMILY_IPV4 : DOTNET_PAL_FAMILY_IPV6) && memcmp(entry.address.address, address, size) == 0) { *index = entry.interface_index; return true; }
    }
    return false;
}
static int32_t membership(intptr_t socket, const dotnet_pal_socket_address *group, uint32_t interface_index, bool join) {
    const dotnet_pal_sockets_ops *s; const dotnet_pal_network_ops *n = sn_network(); int32_t error;
    sn_object *object = socket_of(socket, &s, &error);
    if (!object) return error;
    error = n && n->membership ? net_error(n->membership(object->handle, group, interface_index, join ? 1 : 0)) : Error_ENOTSUP;
    sn_unpin(object);
    return error;
}
PALEXPORT int32_t SystemNative_SetIPv4MulticastOption(intptr_t socket, int32_t multicastOption, IPv4MulticastOption* option) {
    const dotnet_pal_network_ops *n = sn_network();
    if (!option) return Error_EFAULT;
    if (option->InterfaceIndex < 0) return Error_EINVAL;
    uint32_t index = (uint32_t)option->InterfaceIndex;
    if (multicastOption == MulticastOption_MULTICAST_IF) return numeric_option(socket, DOTNET_PAL_SOCKET_MULTICAST_INTERFACE, &(uint64_t){index}, true);
    if (multicastOption != MulticastOption_MULTICAST_ADD && multicastOption != MulticastOption_MULTICAST_DROP) return Error_EINVAL;
    /* Both address fields hold the four bytes in network order. */
    dotnet_pal_socket_address group = {.family = DOTNET_PAL_FAMILY_IPV4};
    memcpy(group.address, &option->MulticastAddress, 4);
    if (index == 0 && option->LocalAddress != 0 && (!n || !interface_of_address(n, (const uint8_t*)&option->LocalAddress, 4, &index))) return Error_EADDRNOTAVAIL;
    return membership(socket, &group, index, multicastOption == MulticastOption_MULTICAST_ADD);
}
PALEXPORT int32_t SystemNative_SetIPv6MulticastOption(intptr_t socket, int32_t multicastOption, IPv6MulticastOption* option) {
    if (!option) return Error_EFAULT;
    if (option->InterfaceIndex < 0 || !option->Address.IsIPv6) return Error_EINVAL;
    if (multicastOption != MulticastOption_MULTICAST_ADD && multicastOption != MulticastOption_MULTICAST_DROP) return Error_EINVAL;
    dotnet_pal_socket_address group = {.family = DOTNET_PAL_FAMILY_IPV6};
    memcpy(group.address, option->Address.Address, 16);
    return membership(socket, &group, (uint32_t)option->InterfaceIndex, multicastOption == MulticastOption_MULTICAST_ADD);
}
/* A socket does not say which groups it has joined, here or in the reference implementation's kernel. */
PALEXPORT int32_t SystemNative_GetIPv4MulticastOption(intptr_t socket, int32_t multicastOption, IPv4MulticastOption* option) {
    uint64_t index = 0;
    if (!option) return Error_EFAULT;
    memset(option, 0, sizeof *option);
    if (multicastOption != MulticastOption_MULTICAST_IF) return Error_ENOPROTOOPT;
    int32_t error = numeric_option(socket, DOTNET_PAL_SOCKET_MULTICAST_INTERFACE, &index, false);
    if (error == Error_SUCCESS) option->InterfaceIndex = index > INT32_MAX ? 0 : (int32_t)index;
    return error;
}
PALEXPORT int32_t SystemNative_GetIPv6MulticastOption(intptr_t socket, int32_t multicastOption, IPv6MulticastOption* option) {
    (void)socket; (void)multicastOption;
    if (!option) return Error_EFAULT;
    memset(option, 0, sizeof *option);
    return Error_ENOPROTOOPT;
}

/* ---- network interfaces --------------------------------------------------------------------------------- */
static uint16_t hardware_type(uint32_t kind) {
    switch (kind) {
        case DOTNET_PAL_INTERFACE_ETHERNET: return NetworkInterfaceType_Ethernet;
        case DOTNET_PAL_INTERFACE_LOOPBACK: return NetworkInterfaceType_Loopback;
        case DOTNET_PAL_INTERFACE_WIRELESS: return NetworkInterfaceType_Wireless80211;
        case DOTNET_PAL_INTERFACE_POINT_TO_POINT: return NetworkInterfaceType_Ppp;
        case DOTNET_PAL_INTERFACE_TUNNEL: return NetworkInterfaceType_Tunnel;
        default: return NetworkInterfaceType_Unknown;
    }
}
/* Counts what the group enumerates, and copies the first `room` entries when out is given; false with errno when
 * the enumeration fails. */
static bool each_interface(const dotnet_pal_network_ops *n, NetworkInterfaceInfo *out, int32_t room, int32_t *count) {
    dotnet_pal_network_interface entry;
    for (*count = 0; *count < 4096 && (!out || *count < room); ++*count) {
        uint32_t status = n->interface_entry ? n->interface_entry((size_t)*count, &entry, sizeof entry) : DOTNET_PAL_NOT_FOUND;
        if (status == DOTNET_PAL_NOT_FOUND) return true;
        if (status != DOTNET_PAL_OK) { errno = sn_errno(status); return false; }
        if (!out) continue;
        NetworkInterfaceInfo *info = &out[*count];
        memset(info, 0, sizeof *info);
        /* The reference structure has room for 15 bytes of name; longer names are cut, as IFNAMSIZ cuts them there. */
        size_t name = strlen((const char*)entry.name);
        memcpy(info->Name, entry.name, name > 15 ? 15 : name);
        info->Speed = entry.speed_bps == 0 || entry.speed_bps > INT64_MAX ? -1 : (int64_t)entry.speed_bps;
        info->InterfaceIndex = entry.index; info->Mtu = entry.mtu > INT32_MAX ? 0 : (int32_t)entry.mtu; info->HardwareType = hardware_type(entry.kind);
        info->OperationalState = entry.state == DOTNET_PAL_LINK_UP ? OperationalStatus_Up : entry.state == DOTNET_PAL_LINK_DOWN ? OperationalStatus_Down : OperationalStatus_Unknown;
        info->NumAddressBytes = (uint8_t)entry.hardware_address_length;
        memcpy(info->AddressBytes, entry.hardware_address, entry.hardware_address_length);
        info->SupportsMulticast = (entry.flags & DOTNET_PAL_INTERFACE_MULTICAST) != 0;
    }
    return true;
}
static bool each_address(const dotnet_pal_network_ops *n, IpAddressInfo *out, int32_t room, int32_t *count) {
    dotnet_pal_network_address entry;
    for (*count = 0; *count < 65536 && (!out || *count < room); ++*count) {
        uint32_t status = n->address_entry ? n->address_entry((size_t)*count, &entry, sizeof entry) : DOTNET_PAL_NOT_FOUND;
        if (status == DOTNET_PAL_NOT_FOUND) return true;
        if (status != DOTNET_PAL_OK) { errno = sn_errno(status); return false; }
        if (!out) continue;
        IpAddressInfo *info = &out[*count];
        memset(info, 0, sizeof *info);
        info->InterfaceIndex = entry.interface_index; info->NumAddressBytes = entry.address.family == DOTNET_PAL_FAMILY_IPV4 ? 4 : 16;
        info->PrefixLength = (uint8_t)entry.prefix_length;
        memcpy(info->AddressBytes, entry.address.address, info->NumAddressBytes);
    }
    return true;
}
/* One block holds both lists: the managed side frees the interface pointer and nothing else. */
PALEXPORT int32_t SystemNative_GetNetworkInterfaces(int32_t* interfaceCount, NetworkInterfaceInfo** interfaceList, int32_t* addressCount, IpAddressInfo** addressList) {
    const dotnet_pal_network_ops *n = sn_network(); int32_t interfaces = 0, addresses = 0;
    if (!interfaceCount || !interfaceList || !addressCount || !addressList) return sn_fail(EFAULT);
    *interfaceCount = 0; *interfaceList = NULL; *addressCount = 0; *addressList = NULL;
    if (!n) return sn_fail(ENOTSUP);
    if (!each_interface(n, NULL, 0, &interfaces) || !each_address(n, NULL, 0, &addresses)) return -1;
    _Static_assert(sizeof(NetworkInterfaceInfo) % _Alignof(IpAddressInfo) == 0, "the address list follows the interface list");
    size_t first_bytes = (size_t)(interfaces ? interfaces : 1) * sizeof(NetworkInterfaceInfo);
    uint8_t *block = SystemNative_Calloc(1, first_bytes + (size_t)addresses * sizeof(IpAddressInfo));
    if (!block) return sn_fail(ENOMEM);
    /* The set may have changed since it was counted: the copy stops at the room there is and reports what it found. */
    if (!each_interface(n, (NetworkInterfaceInfo*)block, interfaces, interfaceCount) || !each_address(n, (IpAddressInfo*)(block + first_bytes), addresses, addressCount)) {
        SystemNative_Free(block); *interfaceCount = 0; *addressCount = 0;
        return -1;
    }
    *interfaceList = (NetworkInterfaceInfo*)block; *addressList = (IpAddressInfo*)(block + first_bytes);
    return 0;
}
PALEXPORT int32_t SystemNative_EnumerateInterfaceAddresses(void* context, IPv4AddressFound onIpv4Found, IPv6AddressFound onIpv6Found, LinkLayerAddressFound onLinkLayerFound) {
    const dotnet_pal_network_ops *n = sn_network();
    if (!n) return sn_fail(ENOTSUP);
    dotnet_pal_network_interface entry; dotnet_pal_network_address address;
    for (size_t i = 0; n->interface_entry && i < 4096; ++i) {
        uint32_t status = n->interface_entry(i, &entry, sizeof entry);
        if (status == DOTNET_PAL_NOT_FOUND) break;
        if (status != DOTNET_PAL_OK) return sn_status(status);
        if (onLinkLayerFound && entry.hardware_address_length > 0) {
            LinkLayerAddressInfo info = {.InterfaceIndex = entry.index, .NumAddressBytes = (uint8_t)entry.hardware_address_length, .HardwareType = hardware_type(entry.kind)};
            memcpy(info.AddressBytes, entry.hardware_address, entry.hardware_address_length);
            onLinkLayerFound(context, (const char*)entry.name, &info);
        }
        for (size_t j = 0; n->address_entry && j < 65536; ++j) {
            status = n->address_entry(j, &address, sizeof address);
            if (status == DOTNET_PAL_NOT_FOUND) break;
            if (status != DOTNET_PAL_OK) return sn_status(status);
            if (address.interface_index != entry.index) continue;
            bool v4 = address.address.family == DOTNET_PAL_FAMILY_IPV4;
            IpAddressInfo info = {.InterfaceIndex = entry.index, .NumAddressBytes = v4 ? 4 : 16, .PrefixLength = (uint8_t)address.prefix_length};
            memcpy(info.AddressBytes, address.address.address, info.NumAddressBytes);
            if (v4 && onIpv4Found) onIpv4Found(context, (const char*)entry.name, &info);
            if (!v4 && onIpv6Found) { uint32_t scope = address.address.scope; onIpv6Found(context, (const char*)entry.name, &info, &scope); }
        }
    }
    return 0;
}
/* Routes are not part of the boundary: an interface has no gateway to report. */
PALEXPORT int32_t SystemNative_EnumerateGatewayAddressesForInterface(void* context, uint32_t interfaceIndex, GatewayAddressFound onGatewayFound) {
    (void)context; (void)interfaceIndex; (void)onGatewayFound;
    return 0;
}
PALEXPORT uint32_t SystemNative_InterfaceNameToIndex(char* interfaceName) {
    const dotnet_pal_network_ops *n = sn_network(); dotnet_pal_network_interface entry;
    if (!interfaceName) { errno = EFAULT; return 0; }
    /* The managed side passes "%name"; the name itself is what an interface has. */
    if (interfaceName[0] == '%') interfaceName++;
    for (size_t i = 0; n && n->interface_entry && i < 4096; ++i) {
        if (n->interface_entry(i, &entry, sizeof entry) != DOTNET_PAL_OK) break;
        if (strcmp((const char*)entry.name, interfaceName) == 0) return entry.index;
    }
    errno = n ? ENXIO : ENOTSUP;
    return 0;
}

/* ---- blocking mode ------------------------------------------------------------------------------------- */
PALEXPORT int32_t SystemNative_FcntlSetIsNonBlocking(intptr_t fd, int32_t isNonBlocking) {
    sn_object *object = sn_pin(fd, 0, 0);
    if (!object) return -1;
    int32_t result = 0;
    if (object->kind == SN_SOCKET) {
        const dotnet_pal_sockets_ops *s = sn_sockets();
        result = sn_status(s ? s->set_blocking(object->handle, isNonBlocking == 0) : DOTNET_PAL_UNSUPPORTED);
        if (result == 0) object->nonblocking = isNonBlocking != 0;
    } else if (object->kind == SN_PIPE) {
        /* Reading without blocking is the process unit's doing; writing to a child keeps waiting for room. */
        if (isNonBlocking && object->open_flags != PAL_O_WRONLY && !sn_pipe_watch(object)) result = sn_fail(ENOMEM);
        else object->nonblocking = isNonBlocking != 0;
    } else if (object->kind != SN_FILE) result = sn_fail(ENOTSUP); /* a file never blocks the way a stream would */
    sn_unpin(object);
    return result;
}
PALEXPORT int32_t SystemNative_FcntlGetIsNonBlocking(intptr_t fd, int32_t* isNonBlocking) {
    if (!isNonBlocking) return sn_fail(EFAULT);
    sn_object *object = sn_pin(fd, 0, 0);
    if (!object) return -1;
    *isNonBlocking = (object->kind == SN_SOCKET || object->kind == SN_PIPE) && object->nonblocking;
    sn_unpin(object);
    return 0;
}

/* ---- readiness events ------------------------------------------------------------------------------------ */
#define EVENTS_HANGUP 0x100 /* delivered as read + write; tracked apart so a hung-up socket reports once */
struct sn_port {
    sn_object *registered;          /* list through port_next, under the table lock */
    uint32_t channel;               /* DOTNET_PAL_NO_CHANNEL on a port of a boundary without sockets */
    void *woken;                    /* that port's waiter sleeps on this auto-reset event instead of in poll */
    bool waiting;
    dotnet_pal_poll_entry *entries; /* the waiter's snapshot; only the waiter touches these three */
    sn_object **objects;
    size_t capacity;
};
static uint64_t channels_in_use; /* under the table lock */
static void unlink_locked(sn_object *object) {
    struct sn_port *port = object->port;
    for (sn_object **link = &port->registered; *link; link = &(*link)->port_next) if (*link == object) { *link = object->port_next; break; }
    object->port = NULL; object->port_next = NULL; object->port_events = 0; object->port_reported = 0;
}
struct sn_port *sn_socket_closing(sn_object *object) { struct sn_port *port = object->port; unlink_locked(object); return port; }
void sn_socket_would_block(sn_object *object, int32_t events) {
    sn_lock();
    struct sn_port *port = object->port;
    bool rearm = port && (object->port_reported & events);
    object->port_reported &= ~events;
    sn_unlock();
    if (rearm) sn_port_wake(port);
}
void sn_port_notify(sn_object *object) {
    sn_lock();
    struct sn_port *port = object->port;
    sn_unlock();
    if (port) sn_port_wake(port);
}
static const dotnet_pal_kernel_ops *events(void) {
    const dotnet_pal_api *a = sn_api();
    return sn_has(a, DOTNET_PAL_KERNEL_API_SIZE, DOTNET_PAL_CAP_EVENTS) ? &a->kernel : NULL;
}
/* Ends the waiter's current round: through its poll channel, or through its event on a boundary without sockets. */
void sn_port_wake(struct sn_port *port) {
    const dotnet_pal_sockets_ops *s = sn_sockets(); const dotnet_pal_kernel_ops *k = events();
    if (port->channel != DOTNET_PAL_NO_CHANNEL) { if (s) (void)s->wake(port->channel); }
    else if (k && port->woken) (void)k->event_set(port->woken);
}
void sn_port_destroy(struct sn_port *port) {
    sn_lock();
    while (port->registered) unlink_locked(port->registered);
    if (port->channel != DOTNET_PAL_NO_CHANNEL) channels_in_use &= ~(UINT64_C(1) << port->channel);
    sn_unlock();
    if (port->woken) { const dotnet_pal_kernel_ops *k = events(); if (k) (void)k->event_destroy(port->woken); }
    SystemNative_Free(port->entries); SystemNative_Free(port->objects); SystemNative_Free(port);
}
/* The BCL creates its event ports, and a thread waiting on each, the first time a program touches the
 * Socket type, before it has asked for a single socket. A boundary without sockets must get through that,
 * or the honest answer to the socket request itself (no such address family) would never be reached. Such a
 * port watches no sockets; its waiter sleeps on an event and serves the pipes of child processes only. */
PALEXPORT int32_t SystemNative_CreateSocketEventPort(intptr_t* port) {
    if (!port) return Error_EFAULT;
    *port = -1;
    struct sn_port *self = SystemNative_Calloc(1, sizeof *self);
    sn_object *object = self ? sn_new(SN_PORT, NULL) : NULL;
    if (!object) { SystemNative_Free(self); return Error_ENOMEM; }
    uint32_t channel = DOTNET_PAL_NO_CHANNEL;
    if (sn_sockets()) {
        sn_lock();
        channel = 0;
        while (channel < DOTNET_PAL_POLL_CHANNELS && (channels_in_use & (UINT64_C(1) << channel))) channel++;
        if (channel < DOTNET_PAL_POLL_CHANNELS) channels_in_use |= UINT64_C(1) << channel;
        sn_unlock();
        if (channel == DOTNET_PAL_POLL_CHANNELS) { SystemNative_Free(self); SystemNative_Free(object); return Error_EMFILE; }
    } else {
        const dotnet_pal_kernel_ops *k = events();
        if (!k || k->event_create(0, 0, &self->woken) != DOTNET_PAL_OK) { SystemNative_Free(self); SystemNative_Free(object); return Error_ENOTSUP; }
    }
    self->channel = channel; object->self = self;
    intptr_t fd = sn_install(object);
    if (fd < 0) return Error_EMFILE;
    *port = fd;
    return Error_SUCCESS;
}
PALEXPORT int32_t SystemNative_CloseSocketEventPort(intptr_t port) {
    sn_object *object = sn_pin(port, SN_PORT, EBADF);
    if (!object) return errno_error();
    int32_t result = SystemNative_Close(port) == 0 ? Error_SUCCESS : errno_error();
    /* A waiter holds the port until it sees that it was closed; the pin keeps the port alive while it is told. */
    sn_port_wake(object->self);
    sn_unpin(object);
    return result;
}
PALEXPORT int32_t SystemNative_CreateSocketEventBuffer(int32_t count, SocketEvent** buffer) {
    if (!buffer || count < 0) return Error_EFAULT;
    *buffer = SystemNative_Calloc((uintptr_t)(count > 0 ? count : 1), sizeof(SocketEvent));
    return *buffer ? Error_SUCCESS : Error_ENOMEM;
}
PALEXPORT int32_t SystemNative_FreeSocketEventBuffer(SocketEvent* buffer) { SystemNative_Free(buffer); return Error_SUCCESS; }
PALEXPORT int32_t SystemNative_TryChangeSocketEventRegistration(intptr_t port, intptr_t socket, int32_t currentEvents, int32_t newEvents, uintptr_t data) {
    const int32_t supported = SocketEvents_SA_READ | SocketEvents_SA_WRITE | SocketEvents_SA_READCLOSE | SocketEvents_SA_CLOSE | SocketEvents_SA_ERROR;
    if ((currentEvents & ~supported) || (newEvents & ~supported)) return Error_EINVAL;
    if (currentEvents == newEvents) return Error_SUCCESS;
    sn_object *owner = sn_pin(port, SN_PORT, EBADF);
    if (!owner) return errno_error();
    /* The BCL reads a child's pipe asynchronously through the same engine, so a pipe registers like a socket. */
    sn_object *object = sn_pin(socket, 0, 0);
    if (!object) { int32_t error = errno_error(); sn_unpin(owner); return error; }
    if (object->kind != SN_SOCKET && object->kind != SN_PIPE) { sn_unpin(object); sn_unpin(owner); return Error_ENOTSOCK; }
    if (object->kind == SN_PIPE && newEvents != SocketEvents_SA_NONE && object->open_flags != PAL_O_WRONLY && !sn_pipe_watch(object)) { sn_unpin(object); sn_unpin(owner); return Error_ENOMEM; }
    int32_t result = Error_SUCCESS;
    sn_lock();
    if ((object->kind == SN_SOCKET && owner->self->channel == DOTNET_PAL_NO_CHANNEL) || (object->port && object->port != owner->self)) result = Error_EINVAL; /* one port per socket, as the engine uses them */
    else if (newEvents == SocketEvents_SA_NONE) { if (object->port) unlink_locked(object); }
    else {
        if (!object->port) { object->port = owner->self; object->port_next = owner->self->registered; owner->self->registered = object; object->port_reported = 0; }
        /* A direction that is newly asked for reports its current state, as a fresh epoll registration does. */
        object->port_reported &= ~(newEvents & ~object->port_events);
        object->port_events = newEvents; object->port_data = data;
    }
    sn_unlock();
    if (result == Error_SUCCESS) sn_port_wake(owner->self);
    sn_unpin(object); sn_unpin(owner);
    return result;
}
static bool reserve_snapshot(struct sn_port *port, size_t wanted) {
    if (wanted <= port->capacity) return true;
    size_t capacity = wanted + 16;
    dotnet_pal_poll_entry *entries = SystemNative_Calloc(capacity, sizeof *entries);
    sn_object **objects = SystemNative_Calloc(capacity, sizeof *objects);
    if (!entries || !objects) { SystemNative_Free(entries); SystemNative_Free(objects); return false; }
    SystemNative_Free(port->entries); SystemNative_Free(port->objects);
    port->entries = entries; port->objects = objects; port->capacity = capacity;
    return true;
}
/* Blocks until at least one event is ready. Each round snapshots the registered sockets (pinning them, so a
 * concurrent close waits for the round to end), polls for the directions not yet delivered, and turns what
 * the level-triggered poll says into edges. A wake ends the round early and the next one sees the change.
 * A registered pipe is not polled: its readiness is what the process unit's reader has buffered, looked at in
 * the snapshot, and the reader wakes the port when that changes. */
PALEXPORT int32_t SystemNative_WaitForSocketEvents(intptr_t port, SocketEvent* buffer, int32_t* count) {
    const dotnet_pal_sockets_ops *s = sn_sockets();
    if (!buffer || !count || *count < 0) return Error_EFAULT;
    int32_t room = *count; *count = 0;
    sn_object *owner = sn_pin(port, SN_PORT, EBADF);
    if (!owner) return errno_error();
    struct sn_port *self = owner->self;
    const dotnet_pal_kernel_ops *k = events();
    bool polls = s && self->channel != DOTNET_PAL_NO_CHANNEL;
    if (!polls && !(k && self->woken)) { sn_unpin(owner); return Error_ENOTSUP; }
    sn_lock();
    bool busy = self->waiting; self->waiting = true;
    sn_unlock();
    if (busy) { sn_unpin(owner); return Error_EBUSY; }
    int32_t result = Error_SUCCESS, produced = 0;
    while (produced == 0 && result == Error_SUCCESS) {
        sn_lock();
        size_t registered = 0;
        for (sn_object *o = self->registered; o; o = o->port_next) registered++;
        bool closed = owner->descriptors == 0;
        sn_unlock();
        if (closed) { result = Error_EBADF; break; }
        if (registered > DOTNET_PAL_MAX_POLL) registered = DOTNET_PAL_MAX_POLL;
        if (!reserve_snapshot(self, registered)) { result = Error_ENOMEM; break; }
        size_t n = 0;
        sn_lock();
        for (sn_object *o = self->registered; o && n < self->capacity && n < DOTNET_PAL_MAX_POLL; o = o->port_next) {
            if (o->kind == SN_PIPE) {
                /* The write end always takes data (a write waits for room); the read end is ready with buffered bytes or at its end. */
                int32_t ready = o->open_flags == PAL_O_WRONLY ? SocketEvents_SA_WRITE : sn_pipe_readable(o) ? SocketEvents_SA_READ : 0;
                int32_t fresh = ready & o->port_events & ~o->port_reported;
                if (fresh && produced < room) { o->port_reported |= fresh; buffer[produced] = (SocketEvent){o->port_data, fresh, 0}; produced++; }
                continue;
            }
            uint32_t wanted = ((o->port_events & SocketEvents_SA_READ) && !(o->port_reported & SocketEvents_SA_READ) ? DOTNET_PAL_POLL_READ : 0)
                | ((o->port_events & SocketEvents_SA_WRITE) && !(o->port_reported & SocketEvents_SA_WRITE) ? DOTNET_PAL_POLL_WRITE : 0);
            /* Nothing left to learn about a socket whose every condition has been delivered. */
            if (wanted == 0 && (o->port_reported & SocketEvents_SA_ERROR) && (o->port_reported & EVENTS_HANGUP)) continue;
            o->references++;
            self->objects[n] = o; self->entries[n] = (dotnet_pal_poll_entry){o->handle, wanted, 0}; n++;
        }
        sn_unlock();
        size_t ready = 0;
        uint32_t status = DOTNET_PAL_OK;
        /* With pipe events in hand the sockets are only looked at; otherwise this is where the waiter sleeps. */
        if (polls) status = s->poll(self->entries, n, produced > 0 ? 0 : DOTNET_PAL_INFINITE_NS, self->channel, &ready);
        else if (produced == 0) status = k->event_wait(self->woken, DOTNET_PAL_INFINITE_NS);
        if (status != DOTNET_PAL_OK) result = net_error(status);
        for (size_t i = 0; i < n; ++i) {
            sn_object *o = self->objects[i]; uint32_t triggered = status == DOTNET_PAL_OK ? self->entries[i].triggered : 0;
            sn_lock();
            if (triggered && o->port == self && produced < room) {
                int32_t events = ((triggered & DOTNET_PAL_POLL_READ) ? SocketEvents_SA_READ : 0) | ((triggered & DOTNET_PAL_POLL_WRITE) ? SocketEvents_SA_WRITE : 0)
                    | ((triggered & DOTNET_PAL_POLL_ERROR) ? SocketEvents_SA_ERROR : 0) | ((triggered & DOTNET_PAL_POLL_HANGUP) ? EVENTS_HANGUP : 0);
                events &= ~o->port_reported;
                o->port_reported |= events;
                /* A hang-up is handled by the ordinary read and write paths, which find the condition themselves. */
                if (events & EVENTS_HANGUP) events = (events & ~EVENTS_HANGUP) | SocketEvents_SA_READ | SocketEvents_SA_WRITE;
                events &= o->port_events | SocketEvents_SA_ERROR;
                if (events) { buffer[produced] = (SocketEvent){o->port_data, events, 0}; produced++; }
            }
            sn_unlock();
            sn_unpin(o);
        }
    }
    sn_lock();
    self->waiting = false;
    sn_unlock();
    sn_unpin(owner);
    *count = produced;
    return produced > 0 ? Error_SUCCESS : result;
}

/* ---- poll ---------------------------------------------------------------------------------------------------- */
/* Sockets are asked through the boundary. The output streams accept data at any time; whether console
 * input is ready cannot be observed through the boundary, and a file is always ready. */
PALEXPORT int32_t SystemNative_Poll(PollEvent* pollEvents, uint32_t eventCount, int32_t milliseconds, uint32_t* triggered) {
    if (!pollEvents || !triggered) return Error_EFAULT;
    *triggered = 0;
    if (eventCount > DOTNET_PAL_MAX_POLL) return Error_EINVAL;
    dotnet_pal_poll_entry *entries = SystemNative_Calloc(eventCount ? eventCount : 1, sizeof *entries);
    sn_object **objects = SystemNative_Calloc(eventCount ? eventCount : 1, sizeof *objects);
    uint32_t *slots = SystemNative_Calloc(eventCount ? eventCount : 1, sizeof *slots);
    int32_t result = entries && objects && slots ? Error_SUCCESS : Error_ENOMEM;
    uint32_t count = 0; size_t sockets = 0; bool waited = false;
    for (uint32_t i = 0; i < eventCount && result == Error_SUCCESS; ++i) {
        pollEvents[i].TriggeredEvents = 0;
        sn_object *object = sn_pin(pollEvents[i].FileDescriptor, 0, 0);
        if (!object) { pollEvents[i].TriggeredEvents = PAL_POLLNVAL; count++; continue; }
        if (object->kind == SN_WATCH) {
            /* Waited for when it is the only entry, which is how FileSystemWatcher asks; beside others it is a check. */
            if (eventCount == 1) waited = true;
            if ((pollEvents[i].Events & PAL_POLLIN) && sn_watch_ready(object, eventCount == 1 ? milliseconds : 0)) { pollEvents[i].TriggeredEvents = PAL_POLLIN; count++; }
            sn_unpin(object);
            continue;
        }
        if (object->kind == SN_SOCKET) {
            objects[sockets] = object; slots[sockets] = i;
            entries[sockets] = (dotnet_pal_poll_entry){object->handle, ((pollEvents[i].Events & PAL_POLLIN) ? DOTNET_PAL_POLL_READ : 0) | ((pollEvents[i].Events & PAL_POLLOUT) ? DOTNET_PAL_POLL_WRITE : 0), 0};
            sockets++;
            continue;
        }
        if (object->kind == SN_PIPE) {
            /* What the reader has buffered, now; a pipe is not waited for here. */
            bool in = object->open_flags != PAL_O_WRONLY;
            int16_t ready = (int16_t)(pollEvents[i].Events & (in ? PAL_POLLIN : PAL_POLLOUT));
            if (in && ready) { bool readable = false; if (sn_pipe_watch(object)) { sn_lock(); readable = sn_pipe_readable(object); sn_unlock(); } if (!readable) ready = 0; }
            if (ready) { pollEvents[i].TriggeredEvents = ready; count++; }
        } else if (object->kind == SN_STREAM && object->stream == 0 && (pollEvents[i].Events & PAL_POLLIN)) result = Error_ENOTSUP;
        else {
            int16_t ready = (int16_t)(pollEvents[i].Events & (object->kind == SN_FILE ? (PAL_POLLIN | PAL_POLLOUT) : PAL_POLLOUT));
            if (object->kind == SN_STREAM && object->stream == 0) ready = 0;
            if (ready) { pollEvents[i].TriggeredEvents = ready; count++; }
        }
        sn_unpin(object);
    }
    if (result == Error_SUCCESS && sockets > 0) {
        const dotnet_pal_sockets_ops *s = sn_sockets(); size_t ready = 0;
        /* Something that is ready already turns the wait into a check. */
        uint64_t timeout = count > 0 ? 0 : milliseconds < 0 ? DOTNET_PAL_INFINITE_NS : (uint64_t)milliseconds * 1000000u;
        uint32_t status = s ? s->poll(entries, sockets, timeout, DOTNET_PAL_NO_CHANNEL, &ready) : DOTNET_PAL_UNSUPPORTED;
        if (status != DOTNET_PAL_OK) result = net_error(status);
        for (size_t i = 0; i < sockets && status == DOTNET_PAL_OK; ++i) {
            uint32_t t = entries[i].triggered;
            int16_t events = (int16_t)(((t & DOTNET_PAL_POLL_READ) ? PAL_POLLIN : 0) | ((t & DOTNET_PAL_POLL_WRITE) ? PAL_POLLOUT : 0)
                | ((t & DOTNET_PAL_POLL_ERROR) ? PAL_POLLERR : 0) | ((t & DOTNET_PAL_POLL_HANGUP) ? PAL_POLLHUP : 0));
            if (events) { pollEvents[slots[i]].TriggeredEvents = events; count++; }
        }
    } else if (result == Error_SUCCESS && count == 0 && milliseconds != 0 && !waited) {
        /* Nothing here can become ready later: the wait is the timeout itself. */
        const dotnet_pal_api *a = sn_api();
        if (milliseconds > 0 && sn_has(a, DOTNET_PAL_SERVICES_API_SIZE, DOTNET_PAL_CAP_SCHEDULER) && a->services.sleep_ns) (void)a->services.sleep_ns((uint64_t)milliseconds * 1000000u);
    }
    for (size_t i = 0; i < sockets; ++i) sn_unpin(objects[i]);
    SystemNative_Free(entries); SystemNative_Free(objects); SystemNative_Free(slots);
    if (result == Error_SUCCESS) *triggered = count;
    return result;
}
/* select(2) is how the BCL polls on macOS only; every other target goes through Poll above. */
PALEXPORT int32_t SystemNative_Select(int* readFds, int readFdsCount, int* writeFds, int writeFdsCount, int* errorFds, int errorFdsCount, int32_t microseconds, int32_t maxFd, int* triggered) {
    (void)readFds; (void)readFdsCount; (void)writeFds; (void)writeFdsCount; (void)errorFds; (void)errorFdsCount; (void)microseconds; (void)maxFd;
    if (triggered) *triggered = 0;
    return Error_ENOTSUP;
}

/* ---- names ------------------------------------------------------------------------------------------------------ */
PALEXPORT int32_t SystemNative_GetHostName(uint8_t* name, int32_t nameLength) {
    const dotnet_pal_sockets_ops *s = sn_sockets(); size_t needed = 0;
    if (!name || nameLength <= 0) return sn_fail(EFAULT);
    if (!s || !s->host_name) return sn_fail(ENOTSUP);
    return sn_status(s->host_name(name, (size_t)nameLength, &needed));
}
PALEXPORT int32_t SystemNative_GetDomainName(uint8_t* name, int32_t nameLength) {
    if (!name || nameLength <= 0) return sn_fail(EFAULT);
    name[0] = 0; /* a host with no domain, which is all the boundary can say */
    return 0;
}
PALEXPORT int32_t SystemNative_GetHostEntryForName(const uint8_t* address, int32_t addressFamily, HostEntry* entry) {
    const dotnet_pal_sockets_ops *s = sn_sockets();
    if (!address || !entry) return GetAddrInfoErrorFlags_EAI_BADARG;
    memset(entry, 0, sizeof *entry);
    uint32_t family = addressFamily == AddressFamily_AF_UNSPEC ? 0 : addressFamily == AddressFamily_AF_INET ? DOTNET_PAL_FAMILY_IPV4 : addressFamily == AddressFamily_AF_INET6 ? DOTNET_PAL_FAMILY_IPV6 : UINT32_MAX;
    if (family == UINT32_MAX) return GetAddrInfoErrorFlags_EAI_FAMILY;
    if (!s || !s->resolve) return GetAddrInfoErrorFlags_EAI_FAIL;
    size_t length = strlen((const char*)address), found = 0;
    dotnet_pal_socket_address results[32];
    uint32_t status = s->resolve(address, length, family, results, 32, &found);
    if (status == DOTNET_PAL_NOT_FOUND || status == DOTNET_PAL_INVALID_ARGUMENT) return GetAddrInfoErrorFlags_EAI_NONAME;
    if (status == DOTNET_PAL_OUT_OF_MEMORY) return GetAddrInfoErrorFlags_EAI_MEMORY;
    if (status == DOTNET_PAL_TIMEOUT || status == DOTNET_PAL_WOULD_BLOCK) return GetAddrInfoErrorFlags_EAI_AGAIN;
    if (status != DOTNET_PAL_OK) return GetAddrInfoErrorFlags_EAI_FAIL;
    entry->IPAddressList = SystemNative_Calloc(found, sizeof(IPAddress));
    entry->CanonicalName = SystemNative_Malloc(length + 1);
    if (!entry->IPAddressList || !entry->CanonicalName) { SystemNative_Free(entry->IPAddressList); SystemNative_Free(entry->CanonicalName); memset(entry, 0, sizeof *entry); return GetAddrInfoErrorFlags_EAI_MEMORY; }
    memcpy(entry->CanonicalName, address, length + 1);
    for (size_t i = 0; i < found; ++i) {
        IPAddress *out = &entry->IPAddressList[i];
        out->IsIPv6 = results[i].family == DOTNET_PAL_FAMILY_IPV6; out->ScopeId = results[i].scope;
        memcpy(out->Address, results[i].address, out->IsIPv6 ? 16 : 4);
    }
    entry->IPAddressCount = (int32_t)found;
    return GetAddrInfoErrorFlags_EAI_SUCCESS;
}
PALEXPORT void SystemNative_FreeHostEntry(HostEntry* entry) {
    if (!entry) return;
    SystemNative_Free(entry->CanonicalName); SystemNative_Free(entry->IPAddressList);
    memset(entry, 0, sizeof *entry);
}
/* The name of an address, through the network group. Service names and numeric forms are the BCL's own business. */
PALEXPORT int32_t SystemNative_GetNameInfo(const uint8_t* address, int32_t addressLength, int8_t isIPv6, uint8_t* host, int32_t hostLength, uint8_t* service, int32_t serviceLength, int32_t flags) {
    const dotnet_pal_network_ops *n = sn_network(); size_t needed = 0;
    if (service && serviceLength > 0) service[0] = 0;
    if (host && hostLength > 0) host[0] = 0;
    if (!address || !host || hostLength <= 0 || addressLength != (isIPv6 ? 16 : 4)) return GetAddrInfoErrorFlags_EAI_BADARG;
    if (flags & GetAddrInfoErrorFlags_NI_NUMERICHOST) return GetAddrInfoErrorFlags_EAI_BADFLAGS;
    if (!n || !n->reverse_lookup) return GetAddrInfoErrorFlags_EAI_NONAME;
    dotnet_pal_socket_address of = {.family = isIPv6 ? DOTNET_PAL_FAMILY_IPV6 : DOTNET_PAL_FAMILY_IPV4};
    memcpy(of.address, address, (size_t)addressLength);
    uint32_t status = n->reverse_lookup(&of, host, (size_t)hostLength, &needed);
    if (status == DOTNET_PAL_OK) return GetAddrInfoErrorFlags_EAI_SUCCESS;
    host[0] = 0;
    if (status == DOTNET_PAL_NOT_FOUND || status == DOTNET_PAL_UNSUPPORTED) return GetAddrInfoErrorFlags_EAI_NONAME;
    if (status == DOTNET_PAL_TIMEOUT) return GetAddrInfoErrorFlags_EAI_AGAIN;
    if (status == DOTNET_PAL_OUT_OF_MEMORY || status == DOTNET_PAL_BUFFER_TOO_SMALL) return GetAddrInfoErrorFlags_EAI_MEMORY;
    return GetAddrInfoErrorFlags_EAI_FAIL;
}

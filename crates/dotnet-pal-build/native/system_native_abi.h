#ifndef DOTNET_PAL_SYSTEM_NATIVE_ABI_H
#define DOTNET_PAL_SYSTEM_NATIVE_ABI_H
/* The managed-facing ABI of System.Native at the audited runtime commit
 * (4271d88e0aebf3d04f188f1334c2220d80555ef6): the error enumeration, the
 * structures and the constants System.Private.CoreLib and the BCL pass to the
 * SystemNative_* entry points. tests/test_system_native_abi.py compares every
 * value below with the pinned headers (src/native/libs/System.Native and
 * src/native/libs/Common) when a checkout is available. */
#include <stddef.h>
#include <stdint.h>

typedef enum {
    Error_SUCCESS = 0,
    Error_E2BIG = 0x10001, Error_EACCES = 0x10002, Error_EADDRINUSE = 0x10003, Error_EADDRNOTAVAIL = 0x10004,
    Error_EAFNOSUPPORT = 0x10005, Error_EAGAIN = 0x10006, Error_EALREADY = 0x10007, Error_EBADF = 0x10008,
    Error_EBADMSG = 0x10009, Error_EBUSY = 0x1000A, Error_ECANCELED = 0x1000B, Error_ECHILD = 0x1000C,
    Error_ECONNABORTED = 0x1000D, Error_ECONNREFUSED = 0x1000E, Error_ECONNRESET = 0x1000F, Error_EDEADLK = 0x10010,
    Error_EDESTADDRREQ = 0x10011, Error_EDOM = 0x10012, Error_EDQUOT = 0x10013, Error_EEXIST = 0x10014,
    Error_EFAULT = 0x10015, Error_EFBIG = 0x10016, Error_EHOSTUNREACH = 0x10017, Error_EIDRM = 0x10018,
    Error_EILSEQ = 0x10019, Error_EINPROGRESS = 0x1001A, Error_EINTR = 0x1001B, Error_EINVAL = 0x1001C,
    Error_EIO = 0x1001D, Error_EISCONN = 0x1001E, Error_EISDIR = 0x1001F, Error_ELOOP = 0x10020,
    Error_EMFILE = 0x10021, Error_EMLINK = 0x10022, Error_EMSGSIZE = 0x10023, Error_EMULTIHOP = 0x10024,
    Error_ENAMETOOLONG = 0x10025, Error_ENETDOWN = 0x10026, Error_ENETRESET = 0x10027, Error_ENETUNREACH = 0x10028,
    Error_ENFILE = 0x10029, Error_ENOBUFS = 0x1002A, Error_ENODEV = 0x1002C, Error_ENOENT = 0x1002D,
    Error_ENOEXEC = 0x1002E, Error_ENOLCK = 0x1002F, Error_ENOLINK = 0x10030, Error_ENOMEM = 0x10031,
    Error_ENOMSG = 0x10032, Error_ENOPROTOOPT = 0x10033, Error_ENOSPC = 0x10034, Error_ENOSYS = 0x10037,
    Error_ENOTCONN = 0x10038, Error_ENOTDIR = 0x10039, Error_ENOTEMPTY = 0x1003A, Error_ENOTRECOVERABLE = 0x1003B,
    Error_ENOTSOCK = 0x1003C, Error_ENOTSUP = 0x1003D, Error_ENOTTY = 0x1003E, Error_ENXIO = 0x1003F,
    Error_EOVERFLOW = 0x10040, Error_EOWNERDEAD = 0x10041, Error_EPERM = 0x10042, Error_EPIPE = 0x10043,
    Error_EPROTO = 0x10044, Error_EPROTONOSUPPORT = 0x10045, Error_EPROTOTYPE = 0x10046, Error_ERANGE = 0x10047,
    Error_EROFS = 0x10048, Error_ESPIPE = 0x10049, Error_ESRCH = 0x1004A, Error_ESTALE = 0x1004B,
    Error_ETIMEDOUT = 0x1004D, Error_ETXTBSY = 0x1004E, Error_EXDEV = 0x1004F, Error_ESOCKTNOSUPPORT = 0x1005E,
    Error_EPFNOSUPPORT = 0x10060, Error_ESHUTDOWN = 0x1006C, Error_EHOSTDOWN = 0x10070, Error_ENODATA = 0x10071,
    Error_EHOSTNOTFOUND = 0x20001, Error_ESOCKETERROR = 0x20002,
    Error_ENONSTANDARD = 0x1FFFF,
} Error;

typedef struct {
    int32_t Flags; int32_t Mode; uint32_t Uid; uint32_t Gid; int64_t Size;
    int64_t ATime; int64_t ATimeNsec; int64_t MTime; int64_t MTimeNsec; int64_t CTime; int64_t CTimeNsec;
    int64_t BirthTime; int64_t BirthTimeNsec; int64_t Dev; int64_t RDev; int64_t Ino; uint32_t UserFlags;
} FileStatus;
enum { FILESTATUS_FLAGS_NONE = 0, FILESTATUS_FLAGS_HAS_BIRTHTIME = 1 };
enum { PAL_S_IFMT = 0xF000, PAL_S_IFIFO = 0x1000, PAL_S_IFCHR = 0x2000, PAL_S_IFDIR = 0x4000, PAL_S_IFREG = 0x8000, PAL_S_IFLNK = 0xA000, PAL_S_IFSOCK = 0xC000 };
enum { PAL_O_RDONLY = 0x0000, PAL_O_WRONLY = 0x0001, PAL_O_RDWR = 0x0002, PAL_O_ACCESS_MODE_MASK = 0x000F,
       PAL_O_CLOEXEC = 0x0010, PAL_O_CREAT = 0x0020, PAL_O_EXCL = 0x0040, PAL_O_TRUNC = 0x0080, PAL_O_SYNC = 0x0100, PAL_O_NOFOLLOW = 0x0200 };
enum { PAL_DT_UNKNOWN = 0, PAL_DT_FIFO = 1, PAL_DT_CHR = 2, PAL_DT_DIR = 4, PAL_DT_BLK = 6, PAL_DT_REG = 8, PAL_DT_LNK = 10, PAL_DT_SOCK = 12, PAL_DT_WHT = 14 };
typedef struct { const char* Name; int32_t NameLength; int32_t InodeType; } DirectoryEntry;
enum { PAL_LOCK_SH = 1, PAL_LOCK_EX = 2, PAL_LOCK_NB = 4, PAL_LOCK_UN = 8 };
enum { PAL_SEEK_SET = 0, PAL_SEEK_CUR = 1, PAL_SEEK_END = 2 };
enum { PAL_PROT_NONE = 0, PAL_PROT_READ = 1, PAL_PROT_WRITE = 2, PAL_PROT_EXEC = 4 };
enum { PAL_MAP_SHARED = 0x01, PAL_MAP_PRIVATE = 0x02, PAL_MAP_ANONYMOUS = 0x10 };
enum { PAL_MS_ASYNC = 0x01, PAL_MS_SYNC = 0x02, PAL_MS_INVALIDATE = 0x10 };
enum { PAL_SC_CLK_TCK = 1, PAL_SC_PAGESIZE = 2 };
enum { PAL_IN_ACCESS = 0x00000001, PAL_IN_MODIFY = 0x00000002, PAL_IN_ATTRIB = 0x00000004, PAL_IN_MOVED_FROM = 0x00000040, PAL_IN_MOVED_TO = 0x00000080,
       PAL_IN_CREATE = 0x00000100, PAL_IN_DELETE = 0x00000200, PAL_IN_Q_OVERFLOW = 0x00004000, PAL_IN_IGNORED = 0x00008000, PAL_IN_ONLYDIR = 0x01000000,
       PAL_IN_DONT_FOLLOW = 0x02000000, PAL_IN_EXCL_UNLINK = 0x04000000, PAL_IN_ISDIR = 0x40000000 };
typedef struct { uint64_t AvailableFreeSpace; uint64_t TotalFreeSpace; uint64_t TotalSize; } MountPointInformation;
typedef void (*MountPointFound)(void* context, const char* name);
typedef struct { int32_t FileDescriptor; int16_t Events; int16_t TriggeredEvents; } PollEvent;
enum { PAL_POLLIN = 0x0001, PAL_POLLPRI = 0x0002, PAL_POLLOUT = 0x0004, PAL_POLLERR = 0x0008, PAL_POLLHUP = 0x0010, PAL_POLLNVAL = 0x0020 };
typedef struct { uint16_t Row; uint16_t Col; uint16_t XPixel; uint16_t YPixel; } WinSize;
typedef struct { int64_t tv_sec; int64_t tv_nsec; } TimeSpec;
typedef struct { uint64_t lastRecordedCurrentTime; uint64_t lastRecordedKernelTime; uint64_t lastRecordedUserTime; } ProcessCpuInformation;
typedef struct { char* Name; char* Password; uint32_t UserId; uint32_t GroupId; char* UserInfo; char* HomeDirectory; char* Shell; } Passwd;
typedef struct { uint8_t* Base; uintptr_t Count; } IOVector;
typedef enum { PAL_LOG_EMERG = 0, PAL_LOG_ALERT = 1, PAL_LOG_CRIT = 2, PAL_LOG_ERR = 3, PAL_LOG_WARNING = 4, PAL_LOG_NOTICE = 5, PAL_LOG_INFO = 6, PAL_LOG_DEBUG = 7 } SysLogPriority;
/* Networking (pal_networking.h and Common/pal_networking_common.h): the subset the boundary's sockets group can express. */
typedef enum { AddressFamily_AF_UNKNOWN = -1, AddressFamily_AF_UNSPEC = 0, AddressFamily_AF_UNIX = 1, AddressFamily_AF_INET = 2, AddressFamily_AF_INET6 = 23 } AddressFamily;
typedef enum { SocketType_UNKNOWN = -1, SocketType_SOCK_STREAM = 1, SocketType_SOCK_DGRAM = 2, SocketType_SOCK_RAW = 3 } SocketType;
typedef enum { ProtocolType_PT_UNKNOWN = -1, ProtocolType_PT_UNSPECIFIED = 0, ProtocolType_PT_ICMP = 1, ProtocolType_PT_TCP = 6, ProtocolType_PT_UDP = 17,
               ProtocolType_PT_ICMPV6 = 58 } ProtocolType;
typedef enum { SocketShutdown_SHUT_READ = 0, SocketShutdown_SHUT_WRITE = 1, SocketShutdown_SHUT_BOTH = 2 } SocketShutdown;
typedef enum { SocketOptionLevel_SOL_SOCKET = 0xffff, SocketOptionLevel_SOL_IP = 0, SocketOptionLevel_SOL_IPV6 = 41, SocketOptionLevel_SOL_TCP = 6, SocketOptionLevel_SOL_UDP = 17 } SocketOptionLevel;
typedef enum {
    SocketOptionName_SO_REUSEADDR = 0x0004, SocketOptionName_SO_KEEPALIVE = 0x0008, SocketOptionName_SO_BROADCAST = 0x0020, SocketOptionName_SO_LINGER = 0x0080,
    SocketOptionName_SO_EXCLUSIVEADDRUSE = ~SocketOptionName_SO_REUSEADDR, SocketOptionName_SO_SNDBUF = 0x1001, SocketOptionName_SO_RCVBUF = 0x1002,
    SocketOptionName_SO_SNDTIMEO = 0x1005, SocketOptionName_SO_RCVTIMEO = 0x1006, SocketOptionName_SO_ERROR = 0x1007, SocketOptionName_SO_TYPE = 0x1008,
    SocketOptionName_SO_IPV6_V6ONLY = 27, SocketOptionName_SO_TCP_NODELAY = 1,
    SocketOptionName_SO_IP_TTL = 4, SocketOptionName_SO_IP_MULTICAST_IF = 9, SocketOptionName_SO_IP_MULTICAST_TTL = 10, SocketOptionName_SO_IP_MULTICAST_LOOP = 11,
    SocketOptionName_SO_IPV6_HOPLIMIT = 21, SocketOptionName_SO_TCP_KEEPALIVE_RETRYCOUNT = 16, SocketOptionName_SO_TCP_KEEPALIVE_TIME = 3,
    SocketOptionName_SO_TCP_KEEPALIVE_INTERVAL = 17, SocketOptionName_SO_IP_DONTFRAGMENT = 14, SocketOptionName_SO_IP_PKTINFO = 19,
} SocketOptionName;
typedef enum { MulticastOption_MULTICAST_ADD = 0, MulticastOption_MULTICAST_DROP = 1, MulticastOption_MULTICAST_IF = 2 } MulticastOption;
typedef enum { GetAddrInfoErrorFlags_NI_NAMEREQD = 0x1, GetAddrInfoErrorFlags_NI_NUMERICHOST = 0x2 } GetNameInfoFlags;
typedef enum { OperationalStatus_Up = 1, OperationalStatus_Down = 2, OperationalStatus_Unknown = 4 } OperationalStatus;
typedef enum { NetworkInterfaceType_Unknown = 1, NetworkInterfaceType_Ethernet = 6, NetworkInterfaceType_Ppp = 23, NetworkInterfaceType_Loopback = 24,
               NetworkInterfaceType_Wireless80211 = 71, NetworkInterfaceType_Tunnel = 131 } NetworkInterfaceType;
typedef struct { uint32_t InterfaceIndex; uint8_t AddressBytes[8]; uint8_t NumAddressBytes; uint8_t _padding; uint16_t HardwareType; } LinkLayerAddressInfo;
typedef struct { uint32_t InterfaceIndex; uint8_t AddressBytes[16]; uint8_t NumAddressBytes; uint8_t PrefixLength; uint8_t _padding[2]; } IpAddressInfo;
typedef struct { char Name[16]; int64_t Speed; uint32_t InterfaceIndex; int32_t Mtu; uint16_t HardwareType; uint8_t OperationalState; uint8_t NumAddressBytes;
                 uint8_t AddressBytes[8]; uint8_t SupportsMulticast; uint8_t _padding[3]; } NetworkInterfaceInfo;
typedef void (*IPv4AddressFound)(void* context, const char* interfaceName, IpAddressInfo* addressInfo);
typedef void (*IPv6AddressFound)(void* context, const char* interfaceName, IpAddressInfo* info, uint32_t* scopeId);
typedef void (*LinkLayerAddressFound)(void* context, const char* interfaceName, LinkLayerAddressInfo* llAddress);
typedef void (*GatewayAddressFound)(void* context, IpAddressInfo* addressInfo);
typedef enum { SocketFlags_MSG_PEEK = 0x0002, SocketFlags_MSG_DONTWAIT = 0x1000 } SocketFlags;
typedef enum { SocketEvents_SA_NONE = 0x00, SocketEvents_SA_READ = 0x01, SocketEvents_SA_WRITE = 0x02, SocketEvents_SA_READCLOSE = 0x04, SocketEvents_SA_CLOSE = 0x08, SocketEvents_SA_ERROR = 0x10 } SocketEvents;
typedef enum { GetAddrInfoErrorFlags_EAI_SUCCESS = 0, GetAddrInfoErrorFlags_EAI_AGAIN = 1, GetAddrInfoErrorFlags_EAI_BADFLAGS = 2, GetAddrInfoErrorFlags_EAI_FAIL = 3, GetAddrInfoErrorFlags_EAI_FAMILY = 4,
               GetAddrInfoErrorFlags_EAI_NONAME = 5, GetAddrInfoErrorFlags_EAI_BADARG = 6, GetAddrInfoErrorFlags_EAI_MEMORY = 8 } GetAddrInfoErrorFlags;
enum { NUM_BYTES_IN_IPV4_ADDRESS = 4, NUM_BYTES_IN_IPV6_ADDRESS = 16, MAX_IP_ADDRESS_BYTES = 16 };
typedef struct { uint8_t Address[MAX_IP_ADDRESS_BYTES]; uint32_t IsIPv6; uint32_t ScopeId; } IPAddress;
typedef struct { uint8_t* CanonicalName; uint8_t** Aliases; IPAddress* IPAddressList; int32_t IPAddressCount; } HostEntry;
typedef struct { IPAddress Address; int32_t InterfaceIndex; int32_t Padding; } IPPacketInformation;
typedef struct { uint32_t MulticastAddress; uint32_t LocalAddress; int32_t InterfaceIndex; int32_t Padding; } IPv4MulticastOption;
typedef struct { IPAddress Address; int32_t InterfaceIndex; int32_t Padding; } IPv6MulticastOption;
typedef struct { int32_t OnOff; int32_t Seconds; } LingerOption;
typedef struct { uint8_t* SocketAddress; IOVector* IOVectors; uint8_t* ControlBuffer; int32_t SocketAddressLen; int32_t IOVectorCount; int32_t ControlBufferLen; int32_t Flags; } MessageHeader;
typedef struct { uintptr_t Data; int32_t Events; uint32_t Padding; } SocketEvent;
typedef struct LowLevelMonitor LowLevelMonitor;
typedef void (*TerminalInvalidationCallback)(void);
#endif

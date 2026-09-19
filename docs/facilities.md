# Change watching, file mappings, volumes, network information and local sockets

Five capability groups follow `terminal` in `dotnet_pal_api`. The boundary's
System.Native builds `FileSystemWatcher`, `MemoryMappedFile`, `DriveInfo`,
`NetworkInterface`, reverse name lookup, multicast membership, Unix domain sockets and
named pipes on them. A port implements each group as one Rust trait, or leaves it absent.

| Group | Capability bit | Trait | Callbacks |
| --- | --- | --- | --- |
| `watches` | `DOTNET_PAL_CAP_WATCHES` | `port::Watches` | `open`, `close`, `add`, `remove`, `read` |
| `mappings` | `DOTNET_PAL_CAP_MAPPINGS` | `port::Mappings` | `map`, `unmap`, `sync` |
| `volumes` | `DOTNET_PAL_CAP_VOLUMES` | `port::Volumes` | `entry`, `status` |
| `network` | `DOTNET_PAL_CAP_NETWORK` | `port::Network` | `interface_entry`, `address_entry`, `reverse_lookup`, `membership` |
| `local_sockets` | `DOTNET_PAL_CAP_LOCAL_SOCKETS` | `port::LocalSockets` | `bind`, `connect`, `address`, `peer_user` |

`mappings` takes the file handles of the `files` group, and `network` and
`local_sockets` take the socket handles of the `sockets` group, so the type that provides
`Files` also provides `Mappings`, and the type that provides `Sockets` also provides
`Network` and `LocalSockets`.

## Change watching

A watcher is a queue of events. `add` watches one directory or file for the events the
consumer names (`ACCESS`, `MODIFY`, `ATTRIBUTES`, `MOVED_FROM`, `MOVED_TO`, `CREATE`,
`DELETE`) and returns an id that is unique among the watcher's live watches. A node the
watcher already watches keeps its id and takes the new events. A provider reports the
kinds it can observe and answers `UNSUPPORTED` to a request it can observe nothing of. `ONLY_DIRECTORY` makes a path
that is no directory fail with `NOT_DIRECTORY`, and `NO_FOLLOW` watches a symbolic link
at the end of the path instead of its target.

`read` takes one event and waits up to a timeout for it; `TIMEOUT` says none came. An
event names the watch, what happened, and for a watched directory the entry it happened
to. `DIRECTORY` marks an event whose subject is a directory. A rename is `MOVED_FROM`
and `MOVED_TO` with the same non-zero cookie, each reported to the watch of its
directory; a provider that cannot pair the two reports `DELETE` and `CREATE`.
`OVERFLOW`, with watch 0, says events were lost. `remove` ends a watch and queues
`REMOVED` for it, and so does the loss of the watched node. The order in which that
`REMOVED` and the `DELETE` of the parent's watch arrive is the target's.

One thread reads at a time. `add` and `remove` may run beside a read that waits, which
is how a consumer ends that wait: `FileSystemWatcher` stops by removing every watch from
another thread, and the `REMOVED` events wake its reader.

```rust
use dotnet_pal_rs::{port, watches};

// `Queue` stands for the port's own event queue.
unsafe fn read(watcher: *mut c_void, timeout_ns: u64, event: &mut watches::Event) -> port::Result<()> {
    let queue = unsafe { &*(watcher as *const Queue) };
    let (watch, kind, name) = queue.pop(timeout_ns).ok_or(port::Error::Timeout)?;
    *event = watches::Event::new(watch, kind, 0, name).ok_or(port::Error::Os)?;
    Ok(())
}
```

System.Native presents a watcher to the BCL as an inotify descriptor. `Read` on it
returns `inotify_event` records, as many as have arrived and fit; `Poll` on it alone
waits for the next event, which `FileSystemWatcher` uses to pair the two halves of a
rename. A read that waits checks once a second whether its descriptor was closed.

## File mappings

`map` makes bytes `[offset, offset + length)` of an open file accessible at the address
it returns. `offset` is a multiple of the page size. Writes through a `SHARED` mapping
reach the file and every other shared mapping of it; writes through a `PRIVATE` one
stay in the mapping. Every mapping needs a handle opened with `READ`, and a shared one
with `WRITE` access a handle opened with `WRITE` as well. The mapping outlives the
handle. It may reach past the end of the file up to the end of the page that holds the
last byte; those bytes read as zero, and what is written to them is lost. What a
mapping that reaches further does is the target's: Linux and macOS grant it and fault
when a page wholly past the end is touched. `sync` returns once the changes made through
a shared mapping are in the file. `unmap` and
`sync` take exactly what one `map` returned.

`SystemNative_MMap` sends a mapping without a descriptor to the `runtime` group's
anonymous memory, as before, and one with a descriptor to this group. It remembers up to
256 file mappings so that `MUnmap` and `MSync`, which receive an address only, reach the
right group. The boundary has no shared memory objects: `MemoryMappedFile.CreateNew`
falls back, as the BCL does wherever `shm_open` is missing, to a temporary file that it
unlinks while it is open. `SysConf(_SC_PAGESIZE)`, which `MemoryMappedFile` and
`Environment.SystemPageSize` ask for, is the page size of the virtual-memory group.

## Volumes

`entry` enumerates the mount points as paths. `status` answers for the volume that
holds a path: its capacity, its free space, the part of the free space this process may
use, and the name of its format (`ext4`, `tmpfs`; empty when the target has no name for
it). `volumes::Status::new` makes the three numbers consistent, because a target takes
them at different moments. `DriveInfo` takes sizes and the format from `status`. On Linux it reads the list of
drives from `/proc/self/mountinfo` through the `files` group and asks the native layer
for the list only where that file does not exist.

## Network information

`interface_entry` and `address_entry` enumerate by index and report `NOT_FOUND` past the
last entry. An interface has the target's index (never 0), a name, a kind (Ethernet,
loopback, wireless, point-to-point, tunnel, unknown), a link state, its MTU and speed
(0 when unknown), a hardware address of up to 8 bytes and a flag for multicast. An
address entry is one IPv4 or IPv6 address with its prefix length and the index of its
interface. `reverse_lookup` writes the host name of an address; `NOT_FOUND` says it has
none, `TIMEOUT` that the answer could not be obtained for now. `membership` joins or
leaves a multicast group on a datagram socket of the group's family; interface 0 lets
the target choose. An IPv4 group on an IPv6 socket that carries both families, which the
BCL uses on a `DualMode` socket, works where the target can do it: Linux takes the IPv4
membership option on such a socket, macOS takes the group as an IPv4-mapped IPv6 address,
and the datagram arrives with an IPv4-mapped sender on both.

`NetworkInterface.GetAllNetworkInterfaces` gets names, types, states, speeds, hardware
addresses and unicast addresses from the two enumerations. Gateways, DNS servers and
statistics are not part of the group: on Linux the BCL reads them from `/proc` and
`/etc/resolv.conf` through the `files` group, and on a target without those files they
are empty. `UdpClient.JoinMulticastGroup` with a local address instead of an interface
index is translated through the address list. The socket options that accompany the
group (`KEEP_ALIVE_IDLE`, `KEEP_ALIVE_INTERVAL`, `KEEP_ALIVE_COUNT`, `HOPS`,
`MULTICAST_HOPS`, `MULTICAST_LOOPBACK`, `MULTICAST_INTERFACE`) belong to the `sockets`
group ([io](io.md)).

## Local sockets

A Unix domain socket is a socket of the `sockets` group, created with
`DOTNET_PAL_FAMILY_LOCAL` as a stream or a datagram socket. Listening, accepting, sending,
receiving, shutting down, polling and the blocking mode work on it as on any socket. The
calls of the `sockets` group that take an IP address refuse it, and the answers that would
hold an address (`accept`, `local_address`, `peer_address`, the sender of a datagram) hold
the bare family. `local_sockets` adds what needs a path: `bind`, which creates the path in
the file system and leaves it there, `connect`, `address` for the path of the socket or of
its peer, and `peer_user`, the numeric user at the other end of a connected stream socket.
A path may have 107 bytes on Linux and 103 on macOS; a longer one is `NAME_TOO_LONG`. The
abstract names of Linux, which live outside the file system, are not carried.

System.Native lays a Unix domain socket address out as the family followed by the path,
which is the layout it reports to the BCL through `GetDomainSocketSizes`.
`NamedPipeServerStream` and `NamedPipeClientStream` are Unix domain sockets below the
temporary directory; `PipeOptions.CurrentUserOnly` and `GetImpersonationUserName` ask
`peer_user`, and the name of that user comes from the `accounts` group
([system](system.md)).

## Providers

| Provider | `Watches` | `Mappings` | `Volumes` | `Network` and `LocalSockets` |
| --- | --- | --- | --- | --- |
| Linux backend (`linux` feature) | `src/linux_watches.rs`: inotify | `src/linux_mappings.rs`: `mmap`, `msync`, `munmap` | `src/linux_volumes.rs`: `/proc/self/mounts` and `statvfs` | `src/linux_network.rs`: `getifaddrs`, `getnameinfo`, the membership socket options; `src/linux_local_sockets.rs`: `AF_UNIX` with `SO_PEERCRED` |
| Desktop `std` port | inotify on Linux; elsewhere a thread that compares directory snapshots | `mmap` on Unix; absent on Windows | as the Linux backend on Linux, `getfsstat` on macOS; absent on Windows | `getifaddrs` on Unix, `socket2` Unix sockets with `SO_PEERCRED` or `getpeereid`; both absent on Windows |
| `dotnet-pal-memfs` | exact events from its own operations; a read waits through the `set_wait` hook | shared mappings point into the file's own pages; private ones are copies | one volume at `/` with the capacity of the file system | |
| C host tables | `host-watches` | `host-mappings` | `host-volumes` | `host-network` (every callback may be NULL), `host-local-sockets` |
| Bare-metal example | through `dotnet-pal-memfs` | through `dotnet-pal-memfs` | through `dotnet-pal-memfs` | absent |

The Linux watch provider is one inotify instance per watcher, with the kernel's watch
descriptor as the id and `IN_EXCL_UNLINK` on every watch. Outside Linux the `std` port
has one thread per watcher that looks at the watched nodes ten times a second and
queues the differences, up to 4096 events. It recognises a rename by device, inode and
birth time, and looks again at once when one half of a rename has no partner yet. It
cannot see a read, so it never reports `ACCESS` and refuses a watch that asks for nothing
else. It cannot see a change that is undone within one interval either, and it ends the
watch of a directory that is renamed, where inotify keeps reporting under the same id.
A full queue drops events in both providers, `REMOVED` included; the reader learns of it
through `OVERFLOW`. The kernel answers `EMFILE` both for a full descriptor table and for
the user's limit of inotify instances; the providers tell the two apart by trying for a
spare descriptor, and report `TOO_MANY_HANDLES` or `NO_SPACE`.

`dotnet-pal-memfs` sees every operation on its tree, so its events are exact and in
order: a file with several names reports under each of them, a replaced destination of a
rename gets no `DELETE`, and a watch follows its directory through a rename, as with
inotify. Its queue holds 1024 events. `no_std` has nothing to block on, so a read that
finds the queue empty releases the tree's lock and calls the hook the port gave to
`set_wait` for at most 10 ms at a time; a port without a scheduler leaves the hook unset
and gets `TIMEOUT` at once. A file moves into page-aligned memory when it is first mapped
shared, and that memory does not move while a shared mapping exists, so the file cannot
grow past the end of its last page until the mapping is gone (`NO_SPACE`). A mapping must
end inside those pages, `EXECUTE` is `UNSUPPORTED`, and access is not enforced on the
memory, because nothing in the crate knows of an MMU.

The Linux volume provider reads `/proc/self/mounts` again for every `entry` call, so an
index can name another mount point once the table has changed. It finds the format of a
path by comparing device numbers with every mount point, which can block on a network
mount that no longer answers.

Both network providers keep one snapshot of the interface and address lists and renew it
when either enumeration starts again at index 0. An interface is a distinct name, with
an alias label cut off. A hardware address longer than 8 bytes is reported as absent.
Linux cannot report the index of the IPv4 multicast interface of a socket, so
`MULTICAST_INTERFACE` reads back what was set through the boundary. A membership needs a
socket of the group's family, or a dual-mode IPv6 socket for an IPv4 group on Linux and
macOS.

## Managed evidence

`samples/FacilitiesProbe` is a C# program that watches a directory tree through
`FileSystemWatcher` (created, changed, renamed and deleted entries, a subdirectory made
while watching, stop and restart), maps files through `MemoryMappedFile` (shared,
copy-on-write and read-only views, a map that grows its file, memory shared between two
views of an anonymous map), reads `DriveInfo`, lists `NetworkInterface`s, looks up the
name of the loopback address, tunes keep-alive and the hop limit of a socket, sends a
datagram to a multicast group it joined and then left, reads gateways, interface
statistics and the UDP listeners (which the BCL takes from `/proc` and `/sys` through the
`files` group), and talks through a Unix domain socket and a named pipe. `scripts/facilities-probe.sh`
publishes it against the source-built runtime and the boundary's System.Native, runs it
on Linux, checks from the link map that every `SystemNative_*` symbol came from the
boundary, and applies the isolation gate. `PROBE_EXPECT` names the groups the probe
expects; for a group that is missing it asserts the failure the BCL documents.
`examples/baremetal-aarch64/build-app.sh FacilitiesProbe` runs the same program with no
OS: `FileSystemWatcher` and `MemoryMappedFile` work on the in-memory file system,
`DriveInfo` reports it as one drive, and network information and local sockets fail as
unsupported. An anonymous `MemoryMappedFile.CreateNew` fails there for another reason: the
BCL names its backing file with a new `Guid`, and that machine has no entropy source.

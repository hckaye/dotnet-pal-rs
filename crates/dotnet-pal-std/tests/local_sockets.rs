//! Exercises the local_sockets group of the std port through the negotiated C table, on
//! whatever desktop OS runs the test: Unix domain sockets of the sockets group bound and
//! connected by path, with the paths and the peer's user compared with what the OS
//! says about the same descriptors.
#[cfg(not(unix))]
#[test]
fn local_sockets_are_absent_without_a_provider() {
    let api = unsafe { &*dotnet_pal_std::api() };
    assert_eq!(api.header.capabilities & dotnet_pal_rs::local_sockets::CAP, 0);
    assert!(api.local_sockets.bind.is_none() && api.local_sockets.connect.is_none() && api.local_sockets.address.is_none() && api.local_sockets.peer_user.is_none());
    // Nothing could bind or connect a local socket, so none is made.
    let mut socket = std::ptr::dangling_mut::<u64>().cast();
    assert_eq!(unsafe { api.sockets.create.unwrap()(dotnet_pal_rs::sockets::LOCAL, dotnet_pal_rs::sockets::STREAM, &mut socket) }, dotnet_pal_rs::UNSUPPORTED);
    assert!(socket.is_null());
}

#[cfg(unix)]
mod unix {
    use dotnet_pal_rs::io::{ACCESS_DENIED, ADDRESS_IN_USE, ALREADY_CONNECTED, BROKEN_PIPE, CONNECTION_REFUSED, IN_PROGRESS, NAME_TOO_LONG, NOT_CONNECTED, NOT_DIRECTORY, WOULD_BLOCK};
    use dotnet_pal_rs::kernel::TIMEOUT;
    use dotnet_pal_rs::local_sockets::{self, Stats};
    use dotnet_pal_rs::runtime::{BUFFER_TOO_SMALL, MAX_NAME, NOT_FOUND};
    use dotnet_pal_rs::sockets::{self, Address, PollEntry, DATAGRAM as UDP, IPV4 as V4, LOCAL, NO_CHANNEL, POLL_HANGUP as HANGUP, POLL_READ as READ, POLL_WRITE as WRITE, STREAM as TCP};
    use dotnet_pal_rs::{INVALID_ARGUMENT, OK, UNSUPPORTED};
    use std::{ffi::c_void, mem::{size_of, zeroed}, os::unix::fs::{FileTypeExt, PermissionsExt}, path::PathBuf, ptr, sync::{atomic::{AtomicU32, Ordering}, Mutex, MutexGuard}, time::{Duration, Instant}};

    const MS: u64 = 1_000_000;
    const LIMIT: u64 = 5000;
    /// The longest path the OS takes: `sun_path` less its terminator (107 on Linux, 103 on macOS).
    const LONGEST: usize = size_of::<libc::sockaddr_un>() - std::mem::offset_of!(libc::sockaddr_un, sun_path) - 1;
    const BARE: Address = Address { family: LOCAL as u16, port: 0, scope: 0, address: [0; 16] };
    /// One test at a time: the counters are the process's, and a descriptor is found by what only one socket has.
    static SERIAL: Mutex<()> = Mutex::new(());
    fn serial() -> MutexGuard<'static, ()> { SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) }
    fn api() -> &'static dotnet_pal_rs::Api {
        let api = unsafe { &*dotnet_pal_std::api() };
        assert_eq!(api.header.capabilities & (local_sockets::CAP | sockets::CAP), local_sockets::CAP | sockets::CAP);
        api
    }
    fn l() -> &'static local_sockets::Ops { &api().local_sockets }
    fn s() -> &'static sockets::Ops { &api().sockets }
    fn counts() -> [u64; 5] {
        let mut out = Stats::default();
        assert_eq!(unsafe { l().read_stats.unwrap()(&mut out, size_of::<Stats>()) }, OK);
        [out.bind_ok, out.connect_ok, out.address_ok, out.peer_ok, out.rejected_or_failed]
    }
    /// Counts what the calls below have to add to the group's counters.
    #[derive(Default)]
    struct Tally([u64; 5]);
    impl Tally {
        fn note(&mut self, status: u32, index: usize) -> u32 { self.0[if status == OK { index } else { 4 }] += 1; status }
        fn bind(&mut self, socket: *mut c_void, path: &[u8]) -> u32 { let status = unsafe { l().bind.unwrap()(socket, path.as_ptr(), path.len()) }; self.note(status, 0) }
        fn connect(&mut self, socket: *mut c_void, path: &[u8]) -> u32 { let status = unsafe { l().connect.unwrap()(socket, path.as_ptr(), path.len()) }; self.note(status, 1) }
        /// `(status, needed, the buffer)`; the buffer starts as 0xAA.
        fn address(&mut self, socket: *mut c_void, peer: u32, capacity: usize) -> (u32, usize, Vec<u8>) {
            let (mut needed, mut buffer) = (7usize, vec![0xAAu8; capacity]);
            let status = unsafe { l().address.unwrap()(socket, peer, if capacity == 0 { ptr::null_mut() } else { buffer.as_mut_ptr() }, capacity, &mut needed) };
            (self.note(status, 2), needed, buffer)
        }
        fn user(&mut self, socket: *mut c_void) -> (u32, u32) {
            let mut user = 7u32;
            let status = unsafe { l().peer_user.unwrap()(socket, &mut user) };
            (self.note(status, 3), user)
        }
        /// The text contract of address: the path with its NUL when it fits, the length it needs either way, and a short buffer cleared.
        fn answers(&mut self, socket: *mut c_void, peer: u32, path: &[u8]) {
            let needed = path.len() + 1;
            let padded = |capacity: usize| { let mut all = path.to_vec(); all.resize(capacity, 0); all };
            assert_eq!(self.address(socket, peer, 256), (OK, needed, padded(256)));
            assert_eq!(self.address(socket, peer, needed), (OK, needed, padded(needed)));
            assert_eq!(self.address(socket, peer, needed - 1), (BUFFER_TOO_SMALL, needed, vec![0; needed - 1]));
            assert_eq!(self.address(socket, peer, 0), (BUFFER_TOO_SMALL, needed, Vec::new()));
        }
        fn refuses(&mut self, socket: *mut c_void, peer: u32, status: u32) { assert_eq!(self.address(socket, peer, 256), (status, 0, vec![0; 256])); }
        fn no_user(&mut self, socket: *mut c_void, status: u32) { assert_eq!(self.user(socket), (status, u32::MAX)); }
        fn settled(self, before: [u64; 5]) { let after = counts(); assert_eq!(after, std::array::from_fn(|i| before[i] + self.0[i]), "counters from {before:?}"); }
    }

    /// A scratch directory with a short name (a path has about a hundred bytes) that goes away with the test.
    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let root = PathBuf::from(format!("/tmp/pal-local-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir(&root).unwrap();
            Self(root)
        }
        fn root(&self) -> Vec<u8> { self.0.to_str().unwrap().as_bytes().to_vec() }
        fn at(&self, name: &str) -> Vec<u8> { self.0.join(name).to_str().unwrap().as_bytes().to_vec() }
        /// A path of exactly `length` bytes inside the directory.
        fn long(&self, length: usize) -> Vec<u8> { let mut path = self.at(""); assert!(path.len() < length); path.resize(length, b'p'); path }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            // A directory a test has locked cannot be emptied as it is.
            if let Ok(entries) = std::fs::read_dir(&self.0) { for entry in entries.flatten() { let _ = std::fs::set_permissions(entry.path(), std::fs::Permissions::from_mode(0o700)); } }
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn text(path: &[u8]) -> &str { std::str::from_utf8(path).unwrap() }
    fn is_socket(path: &[u8]) -> bool { std::fs::symlink_metadata(text(path)).is_ok_and(|m| m.file_type().is_socket()) }
    fn exists(path: &[u8]) -> bool { std::fs::symlink_metadata(text(path)).is_ok() }

    /// What the OS itself says, asked by the test. The descriptor behind a handle is found by what only that socket has.
    mod os {
        use super::*;
        /// The path `getsockname` or `getpeername` reports: `None` when the call fails, empty for a socket without a name.
        pub fn name(fd: i32, peer: bool) -> Option<Vec<u8>> {
            let (mut storage, mut length) = (unsafe { zeroed::<libc::sockaddr_storage>() }, size_of::<libc::sockaddr_storage>() as libc::socklen_t);
            let rc = unsafe { if peer { libc::getpeername(fd, ptr::addr_of_mut!(storage).cast(), &mut length) } else { libc::getsockname(fd, ptr::addr_of_mut!(storage).cast(), &mut length) } };
            if rc != 0 || storage.ss_family as i32 != libc::AF_UNIX { return None; }
            let start = std::mem::offset_of!(libc::sockaddr_un, sun_path);
            let bytes = unsafe { std::slice::from_raw_parts(ptr::addr_of!(storage).cast::<u8>().add(start), (length as usize).saturating_sub(start)) };
            Some(bytes[..bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len())].to_vec())
        }
        /// The descriptor whose own name and peer's name are these.
        pub fn descriptor(own: Option<&[u8]>, peer: Option<&[u8]>) -> i32 {
            let found: Vec<i32> = (0..4096).filter(|&fd| name(fd, false).is_some() && name(fd, false).as_deref() == own.or(Some(&[])) && name(fd, true).as_deref() == peer).collect();
            assert_eq!(found.len(), 1, "descriptors named {:?} with a peer named {:?}", own.map(text), peer.map(text));
            found[0]
        }
        /// The peer's user as the OS reports it: SO_PEERCRED on Linux, getpeereid elsewhere.
        pub fn user(fd: i32) -> Option<u32> {
            #[cfg(any(target_os = "linux", target_os = "android"))]
            {
                let (mut peer, mut length) = (libc::ucred { pid: 0, uid: 0, gid: 0 }, size_of::<libc::ucred>() as libc::socklen_t);
                (unsafe { libc::getsockopt(fd, libc::SOL_SOCKET, libc::SO_PEERCRED, ptr::addr_of_mut!(peer).cast(), &mut length) } == 0).then_some(peer.uid)
            }
            #[cfg(not(any(target_os = "linux", target_os = "android")))]
            {
                let (mut user, mut group) = (0, 0);
                (unsafe { libc::getpeereid(fd, &mut user, &mut group) } == 0).then_some(user)
            }
        }
        pub fn close_on_exec(fd: i32) -> bool { (unsafe { libc::fcntl(fd, libc::F_GETFD) }) & libc::FD_CLOEXEC != 0 }
        pub fn me() -> u32 { unsafe { libc::geteuid() } }
    }

    fn create(family: u32, kind: u32) -> *mut c_void {
        let mut socket = ptr::null_mut();
        assert_eq!(unsafe { s().create.unwrap()(family, kind, &mut socket) }, OK);
        assert!(!socket.is_null());
        // Blocking transfers give up after `LIMIT`: a bug ends in a failed assertion rather than a wait.
        assert_eq!((set_option(socket, sockets::RECEIVE_TIMEOUT, LIMIT), set_option(socket, sockets::SEND_TIMEOUT, LIMIT)), (OK, OK));
        socket
    }
    fn close(socket: *mut c_void) { assert_eq!(unsafe { s().close.unwrap()(socket) }, OK); }
    fn listen(socket: *mut c_void, backlog: u32) { assert_eq!(unsafe { s().listen.unwrap()(socket, backlog) }, OK); }
    fn set_option(socket: *mut c_void, name: u32, value: u64) -> u32 { unsafe { s().set_option.unwrap()(socket, name, value) } }
    fn get_option(socket: *mut c_void, name: u32) -> (u32, u64) { let mut value = 7u64; (unsafe { s().get_option.unwrap()(socket, name, &mut value) }, value) }
    fn set_blocking(socket: *mut c_void, blocking: u32) { assert_eq!(unsafe { s().set_blocking.unwrap()(socket, blocking) }, OK); }
    fn local(socket: *mut c_void) -> (u32, Address) { let mut out = Address { family: 9, ..Address::default() }; (unsafe { s().local_address.unwrap()(socket, &mut out) }, out) }
    fn peer(socket: *mut c_void) -> (u32, Address) { let mut out = Address { family: 9, ..Address::default() }; (unsafe { s().peer_address.unwrap()(socket, &mut out) }, out) }
    fn accept(server: *mut c_void) -> (u32, *mut c_void, Address) {
        let (mut accepted, mut from) = (ptr::dangling_mut::<u64>().cast(), Address { family: 9, ..Address::default() });
        (unsafe { s().accept.unwrap()(server, &mut accepted, &mut from) }, accepted, from)
    }
    fn bits(socket: *mut c_void, requested: u32, timeout_ns: u64) -> u32 {
        let (mut entry, mut ready) = ([PollEntry { socket, requested, triggered: 99 }], 7usize);
        assert_eq!(unsafe { s().poll.unwrap()(entry.as_mut_ptr(), 1, timeout_ns, NO_CHANNEL, &mut ready) }, OK);
        assert_eq!(ready, (entry[0].triggered != 0) as usize);
        entry[0].triggered
    }
    /// The next connection of `server`, waited for on poll, which has a limit on every OS.
    fn accepted(server: *mut c_void) -> *mut c_void {
        assert_eq!(bits(server, READ, LIMIT * MS), READ);
        let (status, socket, from) = accept(server);
        assert_eq!((status, from), (OK, BARE));
        assert!(!socket.is_null());
        assert_eq!((set_option(socket, sockets::RECEIVE_TIMEOUT, LIMIT), set_option(socket, sockets::SEND_TIMEOUT, LIMIT)), (OK, OK));
        socket
    }
    fn send(socket: *mut c_void, data: &[u8], to: Option<&Address>) -> (u32, usize) {
        let mut done = 7usize;
        (unsafe { s().send.unwrap()(socket, if data.is_empty() { ptr::null() } else { data.as_ptr() }, data.len(), to.map_or(ptr::null(), |to| to), &mut done) }, done)
    }
    fn receive(socket: *mut c_void, buffer: &mut [u8], flags: u32) -> (u32, usize, Address) {
        let (mut done, mut from) = (7usize, Address { family: 9, ..Address::default() });
        (unsafe { s().receive.unwrap()(socket, buffer.as_mut_ptr(), buffer.len(), flags, &mut from, &mut done) }, done, from)
    }
    /// Sends `text` one way and expects exactly it on the blocking other side; a stream names no sender.
    fn transfer(from: *mut c_void, to: *mut c_void, text: &[u8]) {
        assert_eq!(send(from, text, None), (OK, text.len()));
        let (mut buffer, mut total) = ([0u8; 64], 0);
        while total < text.len() {
            let (status, done, sender) = receive(to, &mut buffer[total..], 0);
            assert!(status == OK && done > 0 && sender == Address::default(), "status {status} after {total} bytes, sender {sender:?}");
            total += done;
        }
        assert_eq!(&buffer[..total], text);
    }
    const NOT_LOCAL: [u32; 9] = [sockets::NO_DELAY, sockets::IPV6_ONLY, sockets::KEEP_ALIVE_IDLE, sockets::KEEP_ALIVE_INTERVAL, sockets::KEEP_ALIVE_COUNT, sockets::HOPS,
        sockets::MULTICAST_HOPS, sockets::MULTICAST_LOOPBACK, sockets::MULTICAST_INTERFACE];

    #[test]
    fn a_listener_a_client_and_what_each_end_says_about_the_other() {
        let (_serial, scratch, mut t, before) = (serial(), Scratch::new(), Tally::default(), counts());
        let (path, second, client_path) = (scratch.at("listener"), scratch.at("second"), scratch.at("client"));
        let server = create(LOCAL, TCP);
        // Before bind: no path, no peer, and the bare family as the address.
        t.refuses(server, 0, NOT_FOUND);
        t.refuses(server, 1, NOT_CONNECTED);
        t.no_user(server, NOT_CONNECTED);
        assert_eq!((local(server), peer(server)), ((OK, BARE), (NOT_CONNECTED, Address::default())));
        assert_eq!(t.bind(server, &path), OK);
        assert!(is_socket(&path));
        let server_fd = os::descriptor(Some(&path), None);
        assert!(os::close_on_exec(server_fd));
        t.answers(server, 0, &path);
        // A socket has one name: Linux would call its own path taken, and must not be left to create a second one.
        assert_eq!((t.bind(server, &path), t.bind(server, &second), exists(&second)), (INVALID_ARGUMENT, INVALID_ARGUMENT, false));
        listen(server, 8);
        // Linux answers a listener's SO_PEERCRED with the listener's own user; a listener has no peer.
        t.no_user(server, NOT_CONNECTED);
        t.refuses(server, 1, NOT_CONNECTED);

        let client = create(LOCAL, TCP);
        assert_eq!(t.connect(client, &path), OK);
        let accepted_end = accepted(server);
        let (client_fd, accepted_fd) = (os::descriptor(None, Some(&path)), os::descriptor(Some(&path), Some(&[])));
        assert!(os::close_on_exec(client_fd) && os::close_on_exec(accepted_fd));
        for end in [client, accepted_end] { assert_eq!((local(end), peer(end)), ((OK, BARE), (OK, BARE))); }
        assert_eq!(local(server), (OK, BARE));
        // The client has no name of its own; each end names the listener's path where the OS does.
        t.refuses(client, 0, NOT_FOUND);
        t.answers(client, 1, &path);
        t.answers(accepted_end, 0, &path);
        t.refuses(accepted_end, 1, NOT_FOUND);
        assert_eq!((t.user(client), os::user(client_fd)), ((OK, os::me()), Some(os::me())));
        assert_eq!((t.user(accepted_end), os::user(accepted_fd)), ((OK, os::me()), Some(os::me())));
        transfer(client, accepted_end, b"ping");
        transfer(accepted_end, client, b"pong!");
        // The rest of the sockets group works as on any stream: peeking, the bytes that wait, a receive timeout, a half close.
        let mut buffer = [0u8; 16];
        assert_eq!(send(accepted_end, b"again", None), (OK, 5));
        assert_eq!((bits(client, READ, LIMIT * MS), receive(client, &mut buffer, sockets::RECEIVE_PEEK), get_option(client, sockets::AVAILABLE)), (READ, (OK, 5, Address::default()), (OK, 5)));
        assert_eq!((receive(client, &mut buffer, 0), &buffer[..5], get_option(client, sockets::AVAILABLE)), ((OK, 5, Address::default()), &b"again"[..], (OK, 0)));
        assert_eq!(set_option(client, sockets::RECEIVE_TIMEOUT, 100), OK);
        let start = Instant::now();
        assert_eq!(receive(client, &mut buffer, 0), (TIMEOUT, 0, Address::default()));
        assert!(start.elapsed() >= Duration::from_millis(80));
        assert_eq!((set_option(client, sockets::RECEIVE_TIMEOUT, LIMIT), get_option(client, sockets::ERROR)), (OK, (OK, OK as u64)));
        assert_eq!(t.connect(client, &path), ALREADY_CONNECTED);
        assert_eq!(unsafe { s().shutdown.unwrap()(client, sockets::SHUTDOWN_WRITE) }, OK);
        assert_eq!(receive(accepted_end, &mut buffer, 0), (OK, 0, Address::default()));
        assert_eq!((bits(accepted_end, READ, 0), bits(accepted_end, WRITE, 0)), (READ | HANGUP, WRITE));
        transfer(accepted_end, client, b"late");
        assert_eq!(send(client, b"x", None), (BROKEN_PIPE, 0));
        // What a local socket's protocol does not have: TCP tuning, hop limits, the IPv6 switch, multicast.
        for name in NOT_LOCAL { assert_eq!((get_option(client, name), set_option(client, name, 1)), ((UNSUPPORTED, 0), UNSUPPORTED), "option {name}"); }
        close(client);
        close(accepted_end);

        // A client with a path of its own: the accepted end names it as its peer.
        let named = create(LOCAL, TCP);
        assert_eq!((t.bind(named, &client_path), t.connect(named, &path)), (OK, OK));
        let accepted_end = accepted(server);
        assert_eq!(os::name(os::descriptor(Some(&path), Some(&client_path)), true).as_deref(), Some(&client_path[..]));
        t.answers(named, 0, &client_path);
        t.answers(named, 1, &path);
        t.answers(accepted_end, 1, &client_path);
        // The peer's user is the one it connected as, and outlives the peer. Its path does on Linux; macOS no longer has it.
        close(named);
        assert_eq!(t.user(accepted_end), (OK, os::me()));
        if cfg!(any(target_os = "linux", target_os = "android")) { t.answers(accepted_end, 1, &client_path); } else { t.refuses(accepted_end, 1, NOT_CONNECTED); }
        close(accepted_end);

        // Closing removes nothing: the paths stay, nobody listens on them any more, and the test removes them.
        close(server);
        assert!(is_socket(&path) && is_socket(&client_path));
        let late = create(LOCAL, TCP);
        assert_eq!(t.connect(late, &path), CONNECTION_REFUSED);
        close(late);
        t.settled(before);
    }

    #[test]
    fn a_relative_path_and_the_longest_path() {
        let (_serial, scratch, mut t, before) = (serial(), Scratch::new(), Tally::default(), counts());
        let (fits, longer) = (scratch.long(LONGEST), scratch.long(LONGEST + 1));
        let socket = create(LOCAL, TCP);
        assert_eq!((t.bind(socket, &longer), exists(&longer), t.connect(socket, &longer)), (NAME_TOO_LONG, false, NAME_TOO_LONG));
        // The refused bind gave the socket no name.
        t.refuses(socket, 0, NOT_FOUND);
        assert_eq!(t.bind(socket, &fits), OK);
        assert_eq!(os::name(os::descriptor(Some(&fits), None), false).as_deref(), Some(&fits[..]));
        t.answers(socket, 0, &fits);
        listen(socket, 2);
        let client = create(LOCAL, TCP);
        assert_eq!(t.connect(client, &fits), OK);
        t.answers(client, 1, &fits);
        close(client);
        close(socket);
        // A relative path is reported as it was given. It climbs out of the working directory, which belongs to the source tree.
        let depth = std::env::current_dir().unwrap().components().count() - 1;
        let relative = format!("{}{}", "../".repeat(depth), &text(&scratch.at("relative"))[1..]).into_bytes();
        assert!(relative.len() <= LONGEST, "the working directory is too deep for a relative path to the scratch directory");
        let socket = create(LOCAL, UDP);
        assert_eq!(t.bind(socket, &relative), OK);
        assert_eq!(os::name(os::descriptor(Some(&relative), None), false).as_deref(), Some(&relative[..]));
        t.answers(socket, 0, &relative);
        close(socket);
        assert!(is_socket(&relative) && is_socket(&scratch.at("relative")));
        t.settled(before);
    }

    #[test]
    fn a_datagram_pair_connected_by_path() {
        let (_serial, scratch, mut t, before) = (serial(), Scratch::new(), Tally::default(), counts());
        let (a_path, b_path) = (scratch.at("datagram-a"), scratch.at("datagram-b"));
        let (a, b) = (create(LOCAL, UDP), create(LOCAL, UDP));
        assert_eq!((t.bind(a, &a_path), t.connect(b, &a_path)), (OK, OK));
        t.answers(a, 0, &a_path);
        t.answers(b, 1, &a_path);
        t.refuses(b, 0, NOT_FOUND);
        t.refuses(a, 1, NOT_CONNECTED);
        assert_eq!((local(b), peer(b), peer(a)), ((OK, BARE), (OK, BARE), (NOT_CONNECTED, Address::default())));
        // Linux names no sender for a socket without a path, macOS an empty one; the sender is a local socket all the same.
        let mut buffer = [0u8; 16];
        assert_eq!((send(b, b"first", None), bits(a, READ, LIMIT * MS), get_option(a, sockets::AVAILABLE)), ((OK, 5), READ, (OK, 5)));
        assert_eq!((receive(a, &mut buffer, 0), &buffer[..5]), ((OK, 5, BARE), &b"first"[..]));
        // A socket may take its name after it has connected; then the pair is connected both ways.
        assert_eq!((t.bind(b, &b_path), t.connect(a, &b_path)), (OK, OK));
        let (a_fd, b_fd) = (os::descriptor(Some(&a_path), Some(&b_path)), os::descriptor(Some(&b_path), Some(&a_path)));
        assert!(a_fd != b_fd && os::close_on_exec(a_fd));
        t.answers(b, 0, &b_path);
        t.answers(a, 1, &b_path);
        assert_eq!((send(a, b"to b", None), send(b, b"", None)), ((OK, 4), (OK, 0)));
        assert_eq!((receive(b, &mut buffer, 0), &buffer[..4]), ((OK, 4, BARE), &b"to b"[..]));
        // An empty datagram is a real message.
        assert_eq!((bits(a, READ, LIMIT * MS), receive(a, &mut buffer, 0)), (READ, (OK, 0, BARE)));
        set_blocking(a, 0);
        assert_eq!(receive(a, &mut buffer, 0), (WOULD_BLOCK, 0, Address::default()));
        // Credentials belong to a connection: Linux answers a connected datagram socket with user -1, macOS with EINVAL.
        assert!(matches!(os::user(a_fd), None | Some(u32::MAX)));
        t.no_user(a, UNSUPPORTED);
        for name in [sockets::MULTICAST_INTERFACE, sockets::HOPS] { assert_eq!((get_option(a, name), set_option(a, name, 1)), ((UNSUPPORTED, 0), UNSUPPORTED), "option {name}"); }
        // A stream socket cannot connect to a datagram socket's path.
        let other = create(LOCAL, TCP);
        assert_eq!(t.connect(other, &a_path), CONNECTION_REFUSED);
        for socket in [other, a, b] { close(socket); }
        t.settled(before);
    }

    #[test]
    fn readiness_and_the_connects_that_cannot_wait() {
        let (_serial, scratch, mut t, before) = (serial(), Scratch::new(), Tally::default(), counts());
        let path = scratch.at("backlog");
        let server = create(LOCAL, TCP);
        assert_eq!(t.bind(server, &path), OK);
        listen(server, 1);
        set_blocking(server, 0);
        assert_eq!((accept(server), bits(server, READ, 0)), ((WOULD_BLOCK, ptr::null_mut(), Address::default()), 0));
        // A local connect is finished at once or not at all. With the backlog full Linux keeps it waiting, which is WOULD_BLOCK
        // for a socket that cannot wait, and macOS refuses it.
        let (mut clients, mut status) = (Vec::new(), OK);
        while clients.len() < 16 && status == OK {
            let client = create(LOCAL, TCP);
            set_blocking(client, 0);
            status = t.connect(client, &path);
            assert!([OK, IN_PROGRESS, WOULD_BLOCK, CONNECTION_REFUSED].contains(&status), "status {status}");
            if status == IN_PROGRESS { status = OK; }
            clients.push(client);
        }
        let full = if cfg!(any(target_os = "linux", target_os = "android")) { WOULD_BLOCK } else { CONNECTION_REFUSED };
        assert!(status == full && clients.len() >= 2, "status {status} after {} connects", clients.len());
        // A connection the listener has not accepted yet is complete for its client: writable, no pending error, nothing to read.
        let mut byte = [0u8; 1];
        assert_eq!((bits(clients[0], WRITE, LIMIT * MS), get_option(clients[0], sockets::ERROR), bits(clients[0], READ, 0)), (WRITE, (OK, OK as u64), 0));
        assert_eq!(receive(clients[0], &mut byte, 0), (WOULD_BLOCK, 0, Address::default()));
        // A blocking connect waits on Linux, and an expired SEND_TIMEOUT is TIMEOUT, never WOULD_BLOCK.
        let patient = create(LOCAL, TCP);
        assert_eq!(set_option(patient, sockets::SEND_TIMEOUT, 200), OK);
        let start = Instant::now();
        let waited = t.connect(patient, &path);
        if full == WOULD_BLOCK { assert!(waited == TIMEOUT && start.elapsed() >= Duration::from_millis(150), "status {waited} after {:?}", start.elapsed()); } else { assert_eq!(waited, CONNECTION_REFUSED); }
        // The accepted socket starts blocking whatever the listener is: with a receive timeout it waits, then reports the expiry.
        for i in 0..clients.len() - 1 {
            let end = accepted(server);
            if i == 0 {
                assert_eq!(set_option(end, sockets::RECEIVE_TIMEOUT, 100), OK);
                let start = Instant::now();
                assert_eq!(receive(end, &mut byte, 0), (TIMEOUT, 0, Address::default()));
                assert!(start.elapsed() >= Duration::from_millis(80));
                assert_eq!((send(end, b"r", None), bits(clients[0], READ, LIMIT * MS), receive(clients[0], &mut byte, 0), byte[0]), ((OK, 1), READ, (OK, 1, Address::default()), b'r'));
            }
            close(end);
        }
        assert_eq!(accept(server).0, WOULD_BLOCK);
        // Room again: the connect that could not be made succeeds now, as does the patient one.
        for socket in [*clients.last().unwrap(), patient] {
            assert_eq!(t.connect(socket, &path), OK);
            close(accepted(server));
        }
        // The peer is gone: end of stream, reported to a reader as a hangup.
        assert!(bits(patient, READ, LIMIT * MS) & HANGUP != 0);
        assert_eq!(receive(patient, &mut byte, 0), (OK, 0, Address::default()));
        println!("LOCAL SOCKETS note: a listener with a backlog of 1 took {} connects before one answered {}", clients.len() - 1, if full == WOULD_BLOCK { "WOULD_BLOCK" } else { "CONNECTION_REFUSED" });
        for socket in clients.into_iter().chain([patient, server]) { close(socket); }
        t.settled(before);
    }

    #[test]
    fn the_failures_the_file_system_decides() {
        let (_serial, scratch, mut t, before) = (serial(), Scratch::new(), Tally::default(), counts());
        std::fs::write(text(&scratch.at("file")), b"").unwrap();
        let (socket, held) = (create(LOCAL, TCP), create(LOCAL, TCP));
        assert_eq!(t.bind(held, &scratch.at("held")), OK);
        // A path that exists, as a file, as another socket's name or as a directory; then one the file system cannot make.
        assert_eq!((t.bind(socket, &scratch.at("file")), t.bind(socket, &scratch.at("held")), t.bind(socket, &scratch.root())), (ADDRESS_IN_USE, ADDRESS_IN_USE, ADDRESS_IN_USE));
        assert_eq!((t.bind(socket, &scratch.at("missing/socket")), t.bind(socket, &scratch.at("file/socket"))), (NOT_FOUND, NOT_DIRECTORY));
        t.refuses(socket, 0, NOT_FOUND);
        // connect: nothing there, a file that is no socket, a socket nobody listens on, a path through a file.
        assert_eq!((t.connect(socket, &scratch.at("missing")), t.connect(socket, &scratch.at("file")), t.connect(socket, &scratch.at("held"))), (NOT_FOUND, CONNECTION_REFUSED, CONNECTION_REFUSED));
        assert_eq!((t.connect(socket, &scratch.at("file/socket")), t.connect(socket, &scratch.at("missing/socket"))), (NOT_DIRECTORY, NOT_FOUND));
        // Permissions: a directory that takes no new entry, and a listener whose path its user may not write. Root passes both.
        let (locked, guarded) = (scratch.at("locked"), scratch.at("guarded"));
        std::fs::create_dir(text(&locked)).unwrap();
        std::fs::set_permissions(text(&locked), std::fs::Permissions::from_mode(0o555)).unwrap();
        let listener = create(LOCAL, TCP);
        assert_eq!(t.bind(listener, &guarded), OK);
        listen(listener, 4);
        std::fs::set_permissions(text(&guarded), std::fs::Permissions::from_mode(0o000)).unwrap();
        if os::me() == 0 { println!("LOCAL SOCKETS note: running as root, ACCESS_DENIED not checked"); } else {
            assert_eq!((t.bind(socket, &scratch.at("locked/socket")), t.connect(socket, &guarded)), (ACCESS_DENIED, ACCESS_DENIED));
        }
        std::fs::set_permissions(text(&guarded), std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(t.connect(socket, &guarded), OK);
        for socket in [socket, held, listener] { close(socket); }
        t.settled(before);
    }

    #[test]
    fn a_local_socket_has_no_ip_address_and_an_ip_socket_no_path() {
        let (_serial, scratch, mut t, before) = (serial(), Scratch::new(), Tally::default(), counts());
        let (stream, datagram, internet, receiver) = (create(LOCAL, TCP), create(LOCAL, UDP), create(V4, UDP), create(V4, UDP));
        let loopback = Address::v4([127, 0, 0, 1], 0);
        assert_eq!(unsafe { s().bind.unwrap()(receiver, &loopback) }, OK);
        let at = local(receiver).1;
        for socket in [stream, datagram] {
            assert_eq!((unsafe { s().bind.unwrap()(socket, &loopback) }, unsafe { s().connect.unwrap()(socket, &at) }, send(socket, b"x", Some(&at))), (INVALID_ARGUMENT, INVALID_ARGUMENT, (INVALID_ARGUMENT, 0)));
            // Nothing of that gave the socket a name.
            t.refuses(socket, 0, NOT_FOUND);
        }
        let path = scratch.at("internet");
        assert_eq!((t.bind(internet, &path), exists(&path), t.connect(internet, &path)), (INVALID_ARGUMENT, false, INVALID_ARGUMENT));
        t.refuses(internet, 0, INVALID_ARGUMENT);
        t.refuses(internet, 1, INVALID_ARGUMENT);
        t.no_user(internet, INVALID_ARGUMENT);
        // The IP socket still works as one.
        assert_eq!((send(internet, b"ip", Some(&at)), bits(receiver, READ, LIMIT * MS)), ((OK, 2), READ));
        for socket in [stream, datagram, internet, receiver] { close(socket); }
        t.settled(before);
    }

    #[test]
    fn malformed_arguments_are_rejected_before_the_provider() {
        let (_serial, _scratch, before) = (serial(), Scratch::new(), counts());
        let (socket, null) = (create(LOCAL, TCP), ptr::null_mut::<c_void>());
        let (b, c, a, u) = (l().bind.unwrap(), l().connect.unwrap(), l().address.unwrap(), l().peer_user.unwrap());
        let mut rejected = 0u64;
        // A path is 1 to MAX_NAME bytes without a NUL.
        let longest = vec![b'a'; MAX_NAME + 1];
        for (path, length) in [(ptr::null(), 1), (b"/s".as_ptr(), 0), (longest.as_ptr(), longest.len()), (b"/a\0b".as_ptr(), 4), (ptr::without_provenance(usize::MAX - 1), 8)] {
            assert_eq!((unsafe { b(socket, path, length) }, unsafe { c(socket, path, length) }), (INVALID_ARGUMENT, INVALID_ARGUMENT));
            rejected += 2;
        }
        assert_eq!((unsafe { b(null, b"/s".as_ptr(), 2) }, unsafe { c(null, b"/s".as_ptr(), 2) }), (INVALID_ARGUMENT, INVALID_ARGUMENT));
        // The longest path the boundary takes reaches the provider, which has a shorter limit of its own.
        assert_eq!((unsafe { b(socket, longest.as_ptr(), MAX_NAME) }, unsafe { c(socket, longest.as_ptr(), MAX_NAME) }), (NAME_TOO_LONG, NAME_TOO_LONG));
        rejected += 4;
        // address: a real place for the length, a buffer that is one, a socket, and peer is 0 or 1.
        let (mut small, mut needed) = ([0xAAu8; 8], 7usize);
        let odd = ptr::addr_of_mut!(needed).cast::<u8>().wrapping_add(1).cast::<usize>();
        assert_eq!((unsafe { a(socket, 0, small.as_mut_ptr(), 8, ptr::null_mut()) }, unsafe { a(socket, 0, small.as_mut_ptr(), 8, odd) }), (INVALID_ARGUMENT, INVALID_ARGUMENT));
        assert_eq!((unsafe { a(socket, 0, ptr::null_mut(), 1, &mut needed) }, unsafe { a(socket, 0, small.as_mut_ptr(), isize::MAX as usize + 1, &mut needed) }), (INVALID_ARGUMENT, INVALID_ARGUMENT));
        assert_eq!((unsafe { a(socket, 0, ptr::without_provenance_mut(usize::MAX - 3), 8, &mut needed) }, needed), (INVALID_ARGUMENT, 7));
        assert_eq!((unsafe { a(null, 0, small.as_mut_ptr(), 8, &mut needed) }, needed, small), (INVALID_ARGUMENT, 0, [0xAA; 8]));
        needed = 7;
        assert_eq!((unsafe { a(socket, 2, small.as_mut_ptr(), 8, &mut needed) }, needed, unsafe { a(socket, u32::MAX, small.as_mut_ptr(), 8, &mut needed) }), (INVALID_ARGUMENT, 0, INVALID_ARGUMENT));
        rejected += 8;
        // peer_user: a real place for the user, which holds no user once the call has seen it.
        let mut user = 7u32;
        let odd = ptr::addr_of_mut!(user).cast::<u8>().wrapping_add(1).cast::<u32>();
        assert_eq!((unsafe { u(socket, ptr::null_mut()) }, unsafe { u(socket, odd) }, user), (INVALID_ARGUMENT, INVALID_ARGUMENT, 7));
        assert_eq!((unsafe { u(null, &mut user) }, user), (INVALID_ARGUMENT, u32::MAX));
        rejected += 3;
        let mut stats = Stats::default();
        assert_eq!((unsafe { l().read_stats.unwrap()(ptr::null_mut(), size_of::<Stats>()) }, unsafe { l().read_stats.unwrap()(&mut stats, size_of::<Stats>() - 1) }), (INVALID_ARGUMENT, INVALID_ARGUMENT));
        // Every rejection was counted as one, and none of them as a success.
        let mut after = before;
        after[4] += rejected;
        assert_eq!(counts(), after);
        close(socket);
    }
}

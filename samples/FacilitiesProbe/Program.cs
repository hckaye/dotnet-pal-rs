using System.Collections.Concurrent;
using System.IO.MemoryMappedFiles;
using System.Net;
using System.Net.NetworkInformation;
using System.Net.Sockets;

// Every check is an observable claim about the BCL running on the boundary's watches, mappings,
// volumes, network and sockets groups. What the port under test does not provide must fail the
// way the BCL documents.
static class Program
{
    private static int s_failures;
    private static void Check(bool ok, string what) { if (!ok) { s_failures++; Console.Error.WriteLine("FAIL " + what); } }
    private static bool Throws<T>(Action action) where T : Exception
    {
        try { action(); return false; }
        catch (T) { return true; }
        catch (Exception e) { Console.Error.WriteLine("unexpected " + e.GetType().Name + ": " + e.Message); return false; }
    }
    private static string Scratch(string name)
    {
        string root = Path.Combine(Path.GetTempPath(), "facilities-probe-" + Environment.ProcessId + "-" + name);
        if (Directory.Exists(root)) Directory.Delete(root, recursive: true);
        Directory.CreateDirectory(root);
        return root;
    }

    // ---- FileSystemWatcher ---------------------------------------------------------------------
    private static void Watches()
    {
        string root = Scratch("watch");
#if EXPECT_WATCHES
        var seen = new ConcurrentQueue<string>();
        bool Saw(string what, int milliseconds = 5000)
        {
            for (int waited = 0; waited <= milliseconds; waited += 20) { if (seen.Contains(what)) return true; Thread.Sleep(20); }
            Console.Error.WriteLine("events so far: " + string.Join(", ", seen));
            return false;
        }
        using (var watcher = new FileSystemWatcher(root) { IncludeSubdirectories = true,
            NotifyFilter = NotifyFilters.FileName | NotifyFilters.DirectoryName | NotifyFilters.LastWrite | NotifyFilters.Size })
        {
            watcher.Created += (_, e) => seen.Enqueue("created " + e.Name);
            watcher.Changed += (_, e) => seen.Enqueue("changed " + e.Name);
            watcher.Deleted += (_, e) => seen.Enqueue("deleted " + e.Name);
            watcher.Renamed += (_, e) => seen.Enqueue("renamed " + e.OldName + " " + e.Name);
            watcher.Error += (_, e) => seen.Enqueue("error " + e.GetException().Message);
            watcher.EnableRaisingEvents = true;

            File.WriteAllText(Path.Combine(root, "a.txt"), "one");
            Check(Saw("created a.txt"), "no Created for a new file");
            File.AppendAllText(Path.Combine(root, "a.txt"), "two");
            Check(Saw("changed a.txt"), "no Changed for an appended file");
            File.Move(Path.Combine(root, "a.txt"), Path.Combine(root, "b.txt"));
            Check(Saw("renamed a.txt b.txt"), "no Renamed with both names");
            // A directory created under the watched one is watched from then on.
            Directory.CreateDirectory(Path.Combine(root, "sub"));
            Check(Saw("created sub"), "no Created for a directory");
            Thread.Sleep(200);
            File.WriteAllText(Path.Combine(root, "sub", "c.txt"), "three");
            Check(Saw("created " + Path.Combine("sub", "c.txt")), "no Created below a new subdirectory");
            File.Delete(Path.Combine(root, "b.txt"));
            Check(Saw("deleted b.txt"), "no Deleted");

            // Stopping ends the reader thread, which sits in a read without a time limit: only the removal of the watches wakes it.
            watcher.EnableRaisingEvents = false;
            int before = seen.Count;
            File.WriteAllText(Path.Combine(root, "late.txt"), "late");
            Thread.Sleep(400);
            Check(seen.Count == before, "an event arrived after the watcher was stopped: " + string.Join(", ", seen.Skip(before)));
            // And it starts again.
            watcher.EnableRaisingEvents = true;
            File.WriteAllText(Path.Combine(root, "again.txt"), "again");
            Check(Saw("created again.txt"), "no Created after a restart");
            Check(!seen.Any(s => s.StartsWith("error ", StringComparison.Ordinal)), "watcher error: " + string.Join(", ", seen.Where(s => s.StartsWith("error ", StringComparison.Ordinal))));
        }
        Check(Throws<ArgumentException>(() => new FileSystemWatcher(Path.Combine(root, "missing"))), "a watcher on a missing directory");
        Console.WriteLine("watcher created, changed, renamed, deleted and subdirectory events pass, events=" + seen.Count);
#else
        Check(Throws<IOException>(() => { using var watcher = new FileSystemWatcher(root); watcher.EnableRaisingEvents = true; }), "a watcher started without the watches group");
        Console.WriteLine("change watching absent, as expected");
#endif
        Directory.Delete(root, recursive: true);
    }

    // ---- MemoryMappedFile ------------------------------------------------------------------------
    private static void Mappings()
    {
        string root = Scratch("map");
        string path = Path.Combine(root, "mapped.bin");
        int page = Environment.SystemPageSize;
        Check(page >= 4096 && (page & (page - 1)) == 0, "page size " + page);
        byte[] content = new byte[3 * page + 100];
        for (int i = 0; i < content.Length; i++) content[i] = (byte)(i * 7 + 3);
        File.WriteAllBytes(path, content);
#if EXPECT_MAPPINGS
        int at = page + 10;
        using (var map = MemoryMappedFile.CreateFromFile(path, FileMode.Open, null, 0, MemoryMappedFileAccess.ReadWrite))
        {
            using var view = map.CreateViewAccessor(at, 1000);
            Check(view.ReadByte(0) == content[at] && view.ReadByte(999) == content[at + 999], "a view in the middle of the file reads other bytes");
            view.Write(5, (byte)0xAB);
            view.Flush();
            // A second view of the same map sees the write without the file being read again.
            using var other = map.CreateViewStream(0, 0, MemoryMappedFileAccess.Read);
            other.Position = at + 5;
            Check(other.ReadByte() == 0xAB, "a second view does not see the write");
            Check(other.Length >= content.Length, "stream view length " + other.Length);
        }
        Check(File.ReadAllBytes(path)[at + 5] == 0xAB, "the write did not reach the file");
        using (var map = MemoryMappedFile.CreateFromFile(path, FileMode.Open, null, 0, MemoryMappedFileAccess.CopyOnWrite))
        {
            using var view = map.CreateViewAccessor(0, 100, MemoryMappedFileAccess.CopyOnWrite);
            view.Write(0, (byte)0x77);
            Check(view.ReadByte(0) == 0x77, "a copy-on-write view lost its own write");
        }
        Check(File.ReadAllBytes(path)[0] == content[0], "a copy-on-write view wrote to the file");
        using (var map = MemoryMappedFile.CreateFromFile(path, FileMode.Open, null, 0, MemoryMappedFileAccess.Read))
        {
            using var view = map.CreateViewAccessor(0, 0, MemoryMappedFileAccess.Read);
            Check(view.ReadByte(3 * page + 99) == content[3 * page + 99] && !view.CanWrite, "read-only view");
            Check(Throws<UnauthorizedAccessException>(() => map.CreateViewAccessor(0, 10, MemoryMappedFileAccess.ReadWrite)), "a writable view of a read-only map");
        }
        // A map that grows its file, as CreateFromFile with a capacity does.
        using (var map = MemoryMappedFile.CreateFromFile(Path.Combine(root, "grown.bin"), FileMode.Create, null, 2 * page + 1, MemoryMappedFileAccess.ReadWrite))
        {
            using var view = map.CreateViewAccessor();
            view.Write(2 * page, (byte)0x5C);
        }
        byte[] grown = File.ReadAllBytes(Path.Combine(root, "grown.bin"));
        Check(grown.Length == 2 * page + 1 && grown[2 * page] == 0x5C && grown[0] == 0, "a map with a capacity: length " + grown.Length);
        // Memory shared between views without a file of the caller's: the BCL backs it with an unlinked temporary file here.
#if EXPECT_NO_ENTROPY
        // The BCL names that file with a new Guid, which a machine without an entropy source cannot make.
        Check(Throws<System.Security.Cryptography.CryptographicException>(() => MemoryMappedFile.CreateNew(null, 65536)), "an anonymous map without entropy");
#else
        using (var map = MemoryMappedFile.CreateNew(null, 65536))
        {
            using var first = map.CreateViewAccessor();
            using var second = map.CreateViewAccessor(4096, 4096);
            first.Write(4096 + 100, 0x12345678);
            Check(second.ReadInt32(100) == 0x12345678, "two views of an anonymous map do not share memory");
        }
#endif
        Check(Throws<PlatformNotSupportedException>(() => MemoryMappedFile.CreateNew("named", 4096)), "named maps are not a Unix facility");
#if EXPECT_NO_ENTROPY
        Console.WriteLine("memory-mapped files pass: shared, copy-on-write, read-only and grown maps; no anonymous map without entropy");
#else
        Console.WriteLine("memory-mapped files pass: shared, copy-on-write, read-only, grown and anonymous maps");
#endif
#else
        Check(Throws<IOException>(() => { using var map = MemoryMappedFile.CreateFromFile(path, FileMode.Open); using var view = map.CreateViewAccessor(); }), "a view without the mappings group");
        Console.WriteLine("file mapping absent, as expected");
#endif
        Directory.Delete(root, recursive: true);
    }

    // ---- DriveInfo -------------------------------------------------------------------------------------
    private static void Volumes()
    {
#if EXPECT_VOLUMES
        DriveInfo[] drives = DriveInfo.GetDrives();
        Check(drives.Any(d => d.Name == "/"), "the drive list lacks /: " + string.Join(", ", drives.Select(d => d.Name)));
        var top = new DriveInfo("/");
        Check(top.IsReady && top.TotalSize > 0 && top.TotalFreeSpace <= top.TotalSize && top.AvailableFreeSpace <= top.TotalFreeSpace,
            "space of /: " + top.TotalSize + " " + top.TotalFreeSpace + " " + top.AvailableFreeSpace);
        Check(top.DriveFormat.Length > 0, "the format of / is empty");
        var temporary = new DriveInfo(Path.GetTempPath());
        Check(temporary.TotalSize > 0, "space of the temporary directory");
        Check(Throws<DriveNotFoundException>(() => _ = new DriveInfo("/no/such/mount/point").TotalSize), "space of a path that does not exist");
        Console.WriteLine("drives=" + drives.Length + " / is " + top.DriveFormat + " with " + top.TotalSize + " bytes");
#else
        // On Linux the BCL reads the mount list from /proc/self/mountinfo through the files group and asks the native layer only for the sizes.
        Check(File.Exists("/proc/self/mountinfo") || DriveInfo.GetDrives().Length == 0, "drives were listed without the volumes group");
        Check(Throws<IOException>(() => _ = new DriveInfo("/").TotalSize), "the size of / without the volumes group");
        Console.WriteLine("volumes absent, as expected");
#endif
    }

    // ---- interfaces, reverse lookup, multicast, socket options -----------------------------------------
    private static void Network()
    {
#if EXPECT_NETWORK
        NetworkInterface[] interfaces = NetworkInterface.GetAllNetworkInterfaces();
        NetworkInterface? loopback = interfaces.FirstOrDefault(i => i.NetworkInterfaceType == NetworkInterfaceType.Loopback);
        Check(loopback != null && loopback.OperationalStatus == OperationalStatus.Up, "no loopback interface that is up among " + string.Join(", ", interfaces.Select(i => i.Name)));
        if (loopback != null)
        {
            UnicastIPAddressInformation? local = loopback.GetIPProperties().UnicastAddresses.FirstOrDefault(a => a.Address.Equals(IPAddress.Loopback));
            Check(local != null && local.PrefixLength == 8, "127.0.0.1/8 on the loopback interface");
            Check(loopback.GetIPProperties().GetIPv4Properties().Index > 0 && loopback.GetIPProperties().GetIPv4Properties().Mtu > 0, "index and MTU of the loopback interface");
        }
        NetworkInterface? wired = interfaces.FirstOrDefault(i => i.NetworkInterfaceType != NetworkInterfaceType.Loopback && i.OperationalStatus == OperationalStatus.Up && i.SupportsMulticast);
        if (wired != null) Check(wired.GetPhysicalAddress().GetAddressBytes().Length == 6, "hardware address of " + wired.Name);

        string name = Dns.GetHostEntry(IPAddress.Loopback).HostName;
        Check(name.Length > 0 && !IPAddress.TryParse(name, out _), "reverse lookup of 127.0.0.1 gave '" + name + "'");

        // Options the sockets group gained with this group.
        using (var tcp = new Socket(AddressFamily.InterNetwork, SocketType.Stream, ProtocolType.Tcp))
        {
            tcp.SetSocketOption(SocketOptionLevel.Tcp, SocketOptionName.TcpKeepAliveTime, 45);
            tcp.SetSocketOption(SocketOptionLevel.Tcp, SocketOptionName.TcpKeepAliveInterval, 7);
            tcp.SetSocketOption(SocketOptionLevel.Tcp, SocketOptionName.TcpKeepAliveRetryCount, 4);
            tcp.Ttl = 33;
            Check((int)tcp.GetSocketOption(SocketOptionLevel.Tcp, SocketOptionName.TcpKeepAliveTime)! == 45
                && (int)tcp.GetSocketOption(SocketOptionLevel.Tcp, SocketOptionName.TcpKeepAliveInterval)! == 7
                && (int)tcp.GetSocketOption(SocketOptionLevel.Tcp, SocketOptionName.TcpKeepAliveRetryCount)! == 4 && tcp.Ttl == 33, "keep-alive tuning and the hop limit do not read back");
        }

        if (wired == null) { Console.WriteLine("interfaces=" + interfaces.Length + " reverse=" + name + " (no multicast-capable interface: membership not exercised)"); return; }
        int index = wired.GetIPProperties().GetIPv4Properties().Index;
        var group = IPAddress.Parse("239.255.77.78");
        using var receiver = new UdpClient(AddressFamily.InterNetwork);
        receiver.Client.SetSocketOption(SocketOptionLevel.Socket, SocketOptionName.ReuseAddress, true);
        receiver.Client.Bind(new IPEndPoint(IPAddress.Any, 0));
        int port = ((IPEndPoint)receiver.Client.LocalEndPoint!).Port;
        // UdpClient's overloads that take an interface index are for IPv6 only; the option itself takes one for IPv4 as well.
        receiver.Client.SetSocketOption(SocketOptionLevel.IP, SocketOptionName.AddMembership, new MulticastOption(group, index));
        using var sender = new UdpClient(AddressFamily.InterNetwork);
        sender.MulticastLoopback = true;
        sender.Ttl = 1;
        sender.Client.SetSocketOption(SocketOptionLevel.IP, SocketOptionName.MulticastInterface, IPAddress.HostToNetworkOrder(index));
        sender.Send(new byte[] { 1, 2, 3 }, 3, new IPEndPoint(group, port));
        receiver.Client.ReceiveTimeout = 3000;
        IPEndPoint from = new(IPAddress.Any, 0);
        byte[]? got = null;
        try { got = receiver.Receive(ref from); } catch (SocketException e) { Console.Error.WriteLine("multicast receive: " + e.SocketErrorCode); }
        Check(got is { Length: 3 } && got[2] == 3, "a datagram sent to a joined group did not arrive");
        receiver.Client.SetSocketOption(SocketOptionLevel.IP, SocketOptionName.DropMembership, new MulticastOption(group, index));
        sender.Send(new byte[] { 9 }, 1, new IPEndPoint(group, port));
        receiver.Client.ReceiveTimeout = 400;
        Check(Throws<SocketException>(() => receiver.Receive(ref from)), "a datagram arrived after the group was left");
        // Gateways, statistics and listeners are not part of the network group: on Linux the BCL reads them from /proc and
        // /sys, which reach it through the files group.
        int gateways = interfaces.Sum(i => i.GetIPProperties().GatewayAddresses.Count);
        long sentBytes = wired.GetIPStatistics().BytesSent;
        Check(sentBytes > 0, "bytes sent through " + wired.Name + ": " + sentBytes);
        Check(IPGlobalProperties.GetIPGlobalProperties().GetActiveUdpListeners().Any(l => l.Port == port), "the receiver is not among the active UDP listeners");
        Console.WriteLine("gateways=" + gateways + " bytes sent through " + wired.Name + "=" + sentBytes);
        // UdpClient's IPv4 overload names the interface by one of its addresses, which System.Native translates to the index.
        IPAddress? own = wired.GetIPProperties().UnicastAddresses.Select(a => a.Address).FirstOrDefault(a => a.AddressFamily == AddressFamily.InterNetwork);
        if (own != null)
        {
            receiver.JoinMulticastGroup(group, own);
            sender.Send(new byte[] { 4, 5 }, 2, new IPEndPoint(group, port));
            receiver.Client.ReceiveTimeout = 3000;
            got = null;
            try { got = receiver.Receive(ref from); } catch (SocketException e) { Console.Error.WriteLine("multicast receive by address: " + e.SocketErrorCode); }
            Check(got is { Length: 2 } && got[1] == 5, "a datagram sent to a group joined by local address did not arrive");
            receiver.DropMulticastGroup(group);
        }
        Console.WriteLine("interfaces=" + interfaces.Length + " reverse=" + name + " multicast through " + wired.Name + " passes");
#else
        // The BCL reads /etc/resolv.conf before it asks the native layer; on a machine without /etc that read is what fails.
        bool refused;
        try { NetworkInterface.GetAllNetworkInterfaces(); refused = false; }
        catch (NetworkInformationException) { refused = true; }
        catch (DirectoryNotFoundException) { refused = true; }
        Check(refused, "interfaces were listed without the network group");
        Console.WriteLine("network information absent, as expected");
#endif
    }

    // ---- Unix domain sockets and named pipes ------------------------------------------------------------------
    private static void Local()
    {
#if EXPECT_LOCAL
        string root = Scratch("local");
        string path = Path.Combine(root, "probe.sock");
        Check(Socket.OSSupportsUnixDomainSockets, "Unix domain sockets are reported as unsupported");
        using (var listener = new Socket(AddressFamily.Unix, SocketType.Stream, ProtocolType.Unspecified))
        {
            listener.Bind(new UnixDomainSocketEndPoint(path));
            listener.Listen(4);
            Check(File.Exists(path), "the bound path is not in the file system");
            Check(listener.LocalEndPoint is UnixDomainSocketEndPoint bound && bound.ToString() == path, "local end point of the listener: " + listener.LocalEndPoint);
            Task<Socket> accepting = listener.AcceptAsync();
            using var client = new Socket(AddressFamily.Unix, SocketType.Stream, ProtocolType.Unspecified);
            client.ConnectAsync(new UnixDomainSocketEndPoint(path)).Wait(5000);
            Check(accepting.Wait(5000), "the connection was not accepted");
            using Socket server = accepting.Result;
            client.Send("ping"u8);
            byte[] buffer = new byte[16];
            Check(server.Receive(buffer) == 4 && buffer.AsSpan(0, 4).SequenceEqual("ping"u8), "bytes from the client");
            server.SendAsync("pong!"u8.ToArray()).Wait(5000);
            Check(client.ReceiveAsync(buffer).Wait(5000) && buffer.AsSpan(0, 5).SequenceEqual("pong!"u8), "bytes from the server");
            Check(client.RemoteEndPoint is UnixDomainSocketEndPoint remote && remote.ToString() == path, "remote end point of the client: " + client.RemoteEndPoint);
            Check(Throws<SocketException>(() => { using var second = new Socket(AddressFamily.Unix, SocketType.Stream, ProtocolType.Unspecified); second.Bind(new UnixDomainSocketEndPoint(path)); }),
                "a second socket was bound to the same path");
        }
        Check(Throws<SocketException>(() => { using var nobody = new Socket(AddressFamily.Unix, SocketType.Stream, ProtocolType.Unspecified); nobody.Connect(new UnixDomainSocketEndPoint(Path.Combine(root, "missing.sock"))); }),
            "a connection to a path nobody listens on");

        // Named pipes are Unix domain sockets below the temporary directory; CurrentUserOnly asks for the user at the other end.
        string name = "facilities-probe-" + Environment.ProcessId;
        using (var pipeServer = new System.IO.Pipes.NamedPipeServerStream(name, System.IO.Pipes.PipeDirection.InOut, 1, System.IO.Pipes.PipeTransmissionMode.Byte,
            System.IO.Pipes.PipeOptions.Asynchronous | System.IO.Pipes.PipeOptions.CurrentUserOnly))
        {
            Task waiting = pipeServer.WaitForConnectionAsync();
            using var pipeClient = new System.IO.Pipes.NamedPipeClientStream(".", name, System.IO.Pipes.PipeDirection.InOut, System.IO.Pipes.PipeOptions.CurrentUserOnly);
            pipeClient.Connect(5000);
            Check(waiting.Wait(5000), "the named pipe server saw no client");
            pipeClient.Write("question"u8);
            byte[] said = new byte[16];
            Check(pipeServer.Read(said, 0, 8) == 8 && said.AsSpan(0, 8).SequenceEqual("question"u8), "bytes through the named pipe");
            pipeServer.Write("answer"u8);
            Check(pipeClient.Read(said, 0, 6) == 6 && said.AsSpan(0, 6).SequenceEqual("answer"u8), "bytes back through the named pipe");
            Check(pipeServer.GetImpersonationUserName() == Environment.UserName, "the user at the other end of the pipe: " + pipeServer.GetImpersonationUserName());
        }
        Directory.Delete(root, recursive: true);
        Console.WriteLine("Unix domain sockets and named pipes pass");
#else
        Check(!Socket.OSSupportsUnixDomainSockets, "Unix domain sockets are reported without the local sockets group");
        Console.WriteLine("local sockets absent, as expected");
#endif
    }

    // ---- where a datagram arrived, and ICMP through a raw socket --------------------------------------------------
    private static void Packets()
    {
#if EXPECT_PACKETS
        using (var receiver = new Socket(AddressFamily.InterNetwork, SocketType.Dgram, ProtocolType.Udp))
        {
            receiver.Bind(new IPEndPoint(IPAddress.Any, 0));
            receiver.SetSocketOption(SocketOptionLevel.IP, SocketOptionName.PacketInformation, true);
            int port = ((IPEndPoint)receiver.LocalEndPoint!).Port;
            using var sender = new Socket(AddressFamily.InterNetwork, SocketType.Dgram, ProtocolType.Udp);
            sender.SendTo("where"u8.ToArray(), new IPEndPoint(IPAddress.Loopback, port));
            receiver.ReceiveTimeout = 3000;
            byte[] buffer = new byte[64];
            SocketFlags flags = SocketFlags.None;
            EndPoint from = new IPEndPoint(IPAddress.Any, 0);
            int got = receiver.ReceiveMessageFrom(buffer, 0, buffer.Length, ref flags, ref from, out IPPacketInformation where);
            int loopback = NetworkInterface.GetAllNetworkInterfaces().First(i => i.NetworkInterfaceType == NetworkInterfaceType.Loopback).GetIPProperties().GetIPv4Properties().Index;
            Check(got == 5 && where.Address.Equals(IPAddress.Loopback) && where.Interface == loopback, "a datagram to 127.0.0.1 arrived at " + where.Address + " through " + where.Interface);
        }
        // Ping opens a raw ICMP socket where it may, and sends an echo request it built itself.
        using (var ping = new Ping())
        {
            PingReply reply = ping.Send(IPAddress.Loopback, 3000, new byte[] { 1, 2, 3, 4, 5, 6, 7, 8 }, new PingOptions(32, dontFragment: true));
            Check(reply.Status == IPStatus.Success && reply.Address.Equals(IPAddress.Loopback) && reply.Buffer.Length == 8, "ping of the loopback address: " + reply.Status);
        }
        Console.WriteLine("packet information and ICMP echo pass");
#else
        // Without sockets the socket itself is refused; with sockets and without the packets group, the option is.
        Check(Throws<SocketException>(() =>
        {
            using var receiver = new Socket(AddressFamily.InterNetwork, SocketType.Dgram, ProtocolType.Udp);
            receiver.SetSocketOption(SocketOptionLevel.IP, SocketOptionName.PacketInformation, true);
        }), "packet information without the packets group");
        Console.WriteLine("packet information absent, as expected");
#endif
    }

    private static int Main()
    {
        Console.WriteLine("FACILITIES PROBE start");
        Watches();
        Mappings();
        Volumes();
        Network();
        Local();
        Packets();
        Console.WriteLine(s_failures == 0 ? "FACILITIES PROBE PASS" : "FACILITIES PROBE FAIL failures=" + s_failures);
        return s_failures == 0 ? 0 : 1;
    }
}

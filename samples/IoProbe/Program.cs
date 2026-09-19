using System.Net;
using System.Net.Sockets;
using System.Runtime.CompilerServices;
using System.Text;

// Every check is an observable claim about the BCL running on the boundary's files, sockets and
// faults groups. What the port under test does not provide must fail the way the BCL documents.
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

    // ---- hardware faults -------------------------------------------------------------------
    private class Node { public Node? Next; public int Value = 1; public virtual int Read() => Value; }
    private interface IShape { int Sides(); }
    [MethodImpl(MethodImplOptions.NoInlining)] private static Node? NoNode() => null;
    [MethodImpl(MethodImplOptions.NoInlining)] private static int[]? NoArray() => null;
    [MethodImpl(MethodImplOptions.NoInlining)] private static IShape? NoShape() => null;
    [MethodImpl(MethodImplOptions.NoInlining)] private static int ReadField(Node? node) => node!.Value;
    [MethodImpl(MethodImplOptions.NoInlining)] private static void WriteReference(Node? node, Node value) => node!.Next = value;
    [MethodImpl(MethodImplOptions.NoInlining)] private static int Deep(Node? node, int depth) => depth == 0 ? ReadField(node) : Deep(node, depth - 1) + 1;

    private static void Faults()
    {
        // A load, a store of a reference (the write barrier), an array element, a virtual call and an
        // interface call through null: each is a CPU fault in compiled code, not a software check.
        Check(Throws<NullReferenceException>(() => ReadField(NoNode())), "null field load");
        Check(Throws<NullReferenceException>(() => WriteReference(NoNode(), new Node())), "null reference store");
        Check(Throws<NullReferenceException>(() => { _ = NoArray()![3]; }), "null array element");
        Check(Throws<NullReferenceException>(() => NoNode()!.Read()), "null virtual call");
        Check(Throws<NullReferenceException>(() => NoShape()!.Sides()), "null interface call");

        // The exception unwinds like any other: through frames, finally blocks and filters.
        int order = 0; bool filtered = false;
        try
        {
            try { Deep(NoNode(), 16); }
            finally { order = order * 10 + 1; }
        }
        catch (NullReferenceException) when (Flag(ref filtered)) { order = order * 10 + 2; }
        Check(order == 12 && filtered, "fault unwinding order " + order);

        // On another thread, and many times over: the fault path leaves the runtime intact.
        int caught = 0;
        var worker = new Thread(() => { for (int i = 0; i < 200; i++) if (Throws<NullReferenceException>(() => ReadField(NoNode()))) caught++; });
        worker.Start(); worker.Join();
        Check(caught == 200, "faults on a second thread " + caught);
        GC.Collect();
        Check(Task.Run(() => Throws<NullReferenceException>(() => ReadField(NoNode()))).Result, "fault on a pool thread");
        int zero = Zero();
        Check(Throws<DivideByZeroException>(() => { _ = 7 / zero; }), "integer divide by zero");
        Console.WriteLine("faults caught=" + (caught + 6));
    }
    private static bool Flag(ref bool flag) { flag = true; return true; }
    [MethodImpl(MethodImplOptions.NoInlining)] private static int Zero() => 0;

    // ---- files -----------------------------------------------------------------------------
    private static void Files()
    {
#if EXPECT_FILES
        string root = Path.Combine(Path.GetTempPath(), "io-probe-" + Environment.ProcessId);
        if (Directory.Exists(root)) Directory.Delete(root, recursive: true);
        Directory.CreateDirectory(Path.Combine(root, "nested", "deeper"));
        Check(Directory.Exists(root) && Directory.Exists(Path.Combine(root, "nested", "deeper")), "directories created");

        string text = Path.Combine(root, "text.txt");
        File.WriteAllText(text, "first line\n");
        File.AppendAllText(text, "second line\n");
        Check(File.ReadAllText(text) == "first line\nsecond line\n", "text round trip");
        Check(File.ReadAllLines(text).Length == 2, "line count");
        Check(new FileInfo(text).Length == 23, "length " + new FileInfo(text).Length);
        Check(File.GetLastWriteTimeUtc(text).Year >= 2020, "write time " + File.GetLastWriteTimeUtc(text));

        // Positions, sparse growth and truncation through one stream.
        string data = Path.Combine(root, "data.bin");
        using (var stream = new FileStream(data, FileMode.CreateNew, FileAccess.ReadWrite))
        {
            var block = new byte[100_000];
            for (int i = 0; i < block.Length; i++) block[i] = (byte)(i * 31);
            for (int i = 0; i < 12; i++) stream.Write(block);
            stream.Seek(2_000_000, SeekOrigin.Begin);
            stream.WriteByte(0x5a);
            stream.Flush(flushToDisk: true);
            Check(stream.Length == 2_000_001, "sparse length " + stream.Length);
            stream.Position = 1_199_999;
            Check(stream.ReadByte() == unchecked((byte)(99_999 * 31)) && stream.ReadByte() == 0, "data then gap");
            stream.Position = 2_000_000;
            Check(stream.ReadByte() == 0x5a && stream.ReadByte() == -1, "last byte then end");
            stream.SetLength(1000);
            Check(stream.Length == 1000, "truncated length");
        }
        Check(Throws<IOException>(() => new FileStream(data, FileMode.CreateNew).Dispose()), "CreateNew over an existing file");
        long sum = 0;
        foreach (byte b in File.ReadAllBytes(data)) sum += b;
        long expected = 0;
        for (int i = 0; i < 1000; i++) expected += (byte)(i * 31);
        Check(sum == expected, "content checksum");

        // Copy, move with and without replacement, delete.
        string copy = Path.Combine(root, "nested", "copy.bin");
        File.Copy(data, copy);
        Check(new FileInfo(copy).Length == 1000, "copy length");
        Check(Throws<IOException>(() => File.Copy(data, copy)), "copy over an existing file");
        File.Copy(text, copy, overwrite: true);
        Check(File.ReadAllText(copy).StartsWith("first"), "copy with overwrite");
        string moved = Path.Combine(root, "nested", "deeper", "moved.bin");
        File.Move(copy, moved);
        Check(!File.Exists(copy) && File.Exists(moved), "move");
        Check(Throws<IOException>(() => File.Move(data, moved)), "move onto an existing file");
        File.Move(data, moved, overwrite: true);
        Check(!File.Exists(data) && new FileInfo(moved).Length == 1000, "move with overwrite");

        // Enumeration sees exactly what is there.
        for (int i = 0; i < 20; i++) File.WriteAllText(Path.Combine(root, "nested", "item" + i.ToString("D2")), i.ToString());
        var names = Directory.GetFiles(Path.Combine(root, "nested")).Select(Path.GetFileName).OrderBy(n => n, StringComparer.Ordinal).ToArray();
        Check(names.Length == 20 && names[0] == "item00" && names[19] == "item19", "file enumeration " + names.Length);
        var all = Directory.GetFileSystemEntries(root, "*", SearchOption.AllDirectories);
        Check(all.Length == 24, "recursive enumeration " + all.Length);
        Check(Directory.GetDirectories(root).Length == 1, "directory enumeration");

        // Failures are the documented ones.
        Check(!File.Exists(Path.Combine(root, "absent")), "absent file exists");
        Check(Throws<FileNotFoundException>(() => File.ReadAllText(Path.Combine(root, "absent"))), "missing file");
        Check(Throws<DirectoryNotFoundException>(() => File.ReadAllText(Path.Combine(root, "no-such-directory", "file"))), "missing directory");
        Check(Throws<UnauthorizedAccessException>(() => File.ReadAllText(root)), "reading a directory");
        Check(Throws<IOException>(() => Directory.Delete(Path.Combine(root, "nested"))), "deleting a directory that has entries");

        // Asynchronous file I/O runs on the thread pool over the same calls.
        string later = Path.Combine(root, "async.txt");
        File.WriteAllTextAsync(later, new string('x', 50_000)).GetAwaiter().GetResult();
        Check(File.ReadAllTextAsync(later).GetAwaiter().GetResult().Length == 50_000, "asynchronous round trip");

        Directory.Delete(root, recursive: true);
        Check(!Directory.Exists(root), "recursive delete");
        Console.WriteLine("files entries=" + all.Length + " directory=" + Path.GetTempPath());
#else
        // No file system behind the boundary: nothing exists and nothing can be created.
        Check(!File.Exists("/etc/hostname") && !Directory.Exists("/"), "a path exists without a file system");
        Check(Throws<IOException>(() => File.WriteAllText("/probe.txt", "x")) || Throws<UnauthorizedAccessException>(() => File.WriteAllText("/probe.txt", "x")) || Throws<PlatformNotSupportedException>(() => File.WriteAllText("/probe.txt", "x")), "file creation succeeded without a file system");
        Console.WriteLine("files absent, as expected");
#endif
    }

    // ---- sockets ---------------------------------------------------------------------------
    private static void Sockets()
    {
#if EXPECT_SOCKETS
        // Synchronous TCP over loopback: listen, connect, both directions, orderly shutdown.
        using var listener = new Socket(AddressFamily.InterNetwork, SocketType.Stream, ProtocolType.Tcp);
        listener.Bind(new IPEndPoint(IPAddress.Loopback, 0));
        listener.Listen(16);
        var endpoint = (IPEndPoint)listener.LocalEndPoint!;
        Check(endpoint.Port != 0 && endpoint.Address.Equals(IPAddress.Loopback), "listener endpoint " + endpoint);
        var server = new Thread(() =>
        {
            using Socket peer = listener.Accept();
            var buffer = new byte[64];
            int total = 0, got;
            while ((got = peer.Receive(buffer, total, buffer.Length - total, SocketFlags.None)) > 0) total += got;
            Array.Reverse(buffer, 0, total);
            peer.Send(buffer, 0, total, SocketFlags.None);
        });
        server.Start();
        using (var client = new Socket(AddressFamily.InterNetwork, SocketType.Stream, ProtocolType.Tcp))
        {
            client.NoDelay = true;
            Check(client.NoDelay, "NoDelay reads back");
            client.Connect(endpoint);
            Check(((IPEndPoint)client.RemoteEndPoint!).Port == endpoint.Port, "remote endpoint");
            client.Send(Encoding.ASCII.GetBytes("boundary"));
            client.Shutdown(SocketShutdown.Send);
            var reply = new byte[64];
            int total = 0, got;
            while ((got = client.Receive(reply, total, reply.Length - total, SocketFlags.None)) > 0) total += got;
            Check(Encoding.ASCII.GetString(reply, 0, total) == "yradnuob", "synchronous echo '" + Encoding.ASCII.GetString(reply, 0, total) + "'");
        }
        server.Join();

        // A refused connection is the documented SocketError.
        int closedPort;
        using (var temporary = new Socket(AddressFamily.InterNetwork, SocketType.Stream, ProtocolType.Tcp))
        {
            temporary.Bind(new IPEndPoint(IPAddress.Loopback, 0));
            closedPort = ((IPEndPoint)temporary.LocalEndPoint!).Port;
        }
        try { using var refused = new Socket(AddressFamily.InterNetwork, SocketType.Stream, ProtocolType.Tcp); refused.Connect(IPAddress.Loopback, closedPort); Check(false, "connect to a closed port succeeded"); }
        catch (SocketException e) { Check(e.SocketErrorCode == SocketError.ConnectionRefused, "refused connection reported as " + e.SocketErrorCode); }

        // Asynchronous TCP: the socket engine's readiness events come from the boundary's poll.
        Check(AsyncEcho(listener, endpoint).GetAwaiter().GetResult() == 32, "asynchronous echo clients");
        Check(Cancellation(endpoint, listener).GetAwaiter().GetResult(), "cancelled receive");

        // Datagrams carry their sender.
        using (var receiver = new UdpClient(new IPEndPoint(IPAddress.Loopback, 0)))
        using (var sender = new UdpClient(AddressFamily.InterNetwork))
        {
            int port = ((IPEndPoint)receiver.Client.LocalEndPoint!).Port;
            sender.Send(new byte[] { 1, 2, 3, 4, 5 }, 5, new IPEndPoint(IPAddress.Loopback, port));
            Check(receiver.Client.Poll(5_000_000, SelectMode.SelectRead), "datagram readiness");
            IPEndPoint? from = null;
            byte[] datagram = receiver.Receive(ref from);
            Check(datagram.Length == 5 && datagram[4] == 5 && from!.Address.Equals(IPAddress.Loopback), "datagram from " + from);
        }

        Check(Dns.GetHostAddresses("localhost").Any(IPAddress.IsLoopback), "localhost resolves to loopback");
        Check(Throws<SocketException>(() => Dns.GetHostAddresses("no-such-host.invalid")), "unknown host");
        Console.WriteLine("sockets host=" + Dns.GetHostName());
#else
        // No network behind the boundary: no address family exists.
        try { using var socket = new Socket(AddressFamily.InterNetwork, SocketType.Stream, ProtocolType.Tcp); Check(false, "socket created without a network"); }
        catch (SocketException e) { Check(e.SocketErrorCode == SocketError.AddressFamilyNotSupported, "socket creation failed as " + e.SocketErrorCode); }
        Console.WriteLine("sockets absent, as expected");
#endif
    }

#if EXPECT_SOCKETS
    private static async Task<int> AsyncEcho(Socket listener, IPEndPoint endpoint)
    {
        const int Clients = 32;
        var accepting = Task.Run(async () =>
        {
            var sessions = new List<Task>();
            for (int i = 0; i < Clients; i++)
            {
                Socket peer = await listener.AcceptAsync();
                sessions.Add(Task.Run(async () =>
                {
                    using (peer)
                    {
                        var buffer = new byte[4096];
                        int got;
                        while ((got = await peer.ReceiveAsync(buffer, SocketFlags.None)) > 0)
                            for (int sent = 0; sent < got;) sent += await peer.SendAsync(buffer.AsMemory(sent, got - sent), SocketFlags.None);
                    }
                }));
            }
            await Task.WhenAll(sessions);
        });
        int verified = 0;
        var clients = Enumerable.Range(0, Clients).Select(async index =>
        {
            using var client = new TcpClient();
            await client.ConnectAsync(endpoint.Address, endpoint.Port);
            NetworkStream stream = client.GetStream();
            // Larger than the socket buffers, so both sides have to wait for readiness repeatedly.
            var payload = new byte[300_000];
            new Random(index).NextBytes(payload);
            var echoed = new byte[payload.Length];
            Task writing = Task.Run(async () => { await stream.WriteAsync(payload); client.Client.Shutdown(SocketShutdown.Send); });
            int total = 0, got;
            while (total < echoed.Length && (got = await stream.ReadAsync(echoed.AsMemory(total))) > 0) total += got;
            await writing;
            if (total == payload.Length && payload.AsSpan().SequenceEqual(echoed)) Interlocked.Increment(ref verified);
        }).ToArray();
        await Task.WhenAll(clients).WaitAsync(TimeSpan.FromSeconds(120));
        await accepting.WaitAsync(TimeSpan.FromSeconds(120));
        return verified;
    }

    private static async Task<bool> Cancellation(IPEndPoint endpoint, Socket listener)
    {
        using var client = new Socket(AddressFamily.InterNetwork, SocketType.Stream, ProtocolType.Tcp);
        Task<Socket> accepted = listener.AcceptAsync();
        await client.ConnectAsync(endpoint);
        using Socket peer = await accepted;
        using var cancel = new CancellationTokenSource(200);
        try { await client.ReceiveAsync(new byte[16], SocketFlags.None, cancel.Token); return false; }
        catch (OperationCanceledException) { }
        // The socket is still usable after the cancelled wait.
        await peer.SendAsync(new byte[] { 42 }, SocketFlags.None);
        var one = new byte[1];
        return await client.ReceiveAsync(one, SocketFlags.None) == 1 && one[0] == 42;
    }
#endif

    static int Main()
    {
        Console.WriteLine("IO PROBE start");
        Faults();
        Files();
        Sockets();
        Console.WriteLine(s_failures == 0 ? "IO PROBE PASS" : "IO PROBE FAIL count=" + s_failures);
        return s_failures == 0 ? 0 : 1;
    }
}

using System.Diagnostics;
using System.Runtime.CompilerServices;

// Every line this program prints is a claim a reader can check against the
// boundary counters or the port's own log. Nothing here needs a file system or a network.
static class Program
{
    private static int s_finalized;
    private sealed class Finalizable { ~Finalizable() { Interlocked.Increment(ref s_finalized); } }

    [MethodImpl(MethodImplOptions.NoInlining)]
    private static void Allocate(int rounds)
    {
        for (int i = 0; i < rounds; i++)
        {
            var bytes = new byte[1024 + i % 512];
            bytes[0] = (byte)i;
            _ = new Finalizable();
            if (i % 8 == 0) _ = new object[64];
        }
    }

    [MethodImpl(MethodImplOptions.NoInlining)]
    private static int Throwing(int depth)
    {
        if (depth == 0) throw new InvalidOperationException("thrown across " + depth + " frames");
        return Throwing(depth - 1) + 1;
    }

    static int Main(string[] args)
    {
        int failures = 0;
        void Check(bool ok, string what) { if (!ok) { failures++; Console.Error.WriteLine("FAIL " + what); } }

        Console.WriteLine("CONSOLE PROBE start args=" + args.Length + " processors=" + Environment.ProcessorCount);
        Console.Error.WriteLine("stderr line reaches the error stream");

        // Exceptions unwind through managed frames and land in the right handler.
        try { Throwing(12); Check(false, "exception not thrown"); }
        catch (InvalidOperationException e) { Check(e.Message.Contains("frames"), "exception message"); }
        finally { Console.WriteLine("finally ran"); }
        bool filtered = false;
        try { throw new ArgumentException("filter me"); }
        catch (ArgumentException e) when (e.Message == "filter me") { filtered = true; }
        Check(filtered, "exception filter");

        // GC and finalizers.
        long before = GC.GetTotalAllocatedBytes();
        Allocate(20000);
        GC.Collect();
        GC.WaitForPendingFinalizers();
        GC.Collect();
        Check(GC.CollectionCount(0) > 0, "no collections");
        Check(s_finalized > 0, "no finalizers ran");
        Check(GC.GetTotalAllocatedBytes() - before > 20_000_000, "allocation accounting");
        Console.WriteLine("GC collections=" + GC.CollectionCount(0) + " finalized=" + s_finalized);

        // Time and randomness come from the port.
        var watch = Stopwatch.StartNew();
        Thread.Sleep(20);
        watch.Stop();
        Check(watch.ElapsedMilliseconds >= 15, "stopwatch elapsed " + watch.ElapsedMilliseconds);
        var random = new Random();
        int a = random.Next(), b = random.Next();
        Check(a != b || random.Next() != a, "random stream");
        Check(DateTime.UtcNow.Year >= 2020, "wall clock year " + DateTime.UtcNow.Year);

        // Threads, the thread pool and monitors.
        int sum = 0;
        var tasks = Enumerable.Range(1, 4).Select(n => Task.Run(() => { Interlocked.Add(ref sum, n * 1000); Allocate(500); })).ToArray();
        Check(Task.WaitAll(tasks, TimeSpan.FromSeconds(30)), "thread pool tasks");
        Check(sum == 10000, "task sum " + sum);
        var gate = new object();
        bool signaled = false;
        var worker = new Thread(() => { lock (gate) { signaled = true; Monitor.Pulse(gate); } });
        lock (gate) { worker.Start(); Check(Monitor.Wait(gate, 10000), "monitor pulse"); }
        worker.Join();
        Check(signaled, "worker ran");
        Console.WriteLine("threads pool=" + tasks.Length + " monitor=" + signaled);

        // Environment through the boundary. A file that cannot exist fails the same way with or
        // without a file system behind the boundary; samples/IoProbe covers the file system itself.
        string? home = Environment.GetEnvironmentVariable("CONSOLE_PROBE_VALUE");
        Console.WriteLine("environment CONSOLE_PROBE_VALUE=" + (home ?? "<null>"));
        const string Absent = "/console-probe-no-such-directory/file";
        Check(!File.Exists(Absent), "File.Exists said true for a path that cannot exist");
        try { File.ReadAllText(Absent); Check(false, "read a file that cannot exist"); }
        catch (IOException e) { Console.WriteLine("file read failed as expected: " + e.GetType().Name); }
        catch (Exception e) { Console.WriteLine("file read failed as expected: " + e.GetType().Name); }

        string text = string.Join(",", Enumerable.Range(0, 5).Select(i => (i * 1.5).ToString("F1")));
        Check(text == "0.0,1.5,3.0,4.5,6.0", "formatting " + text);
        Check(Math.Round(Math.Sin(Math.PI / 2), 6) == 1 && Math.Log(Math.E) > 0.99, "math");

        Console.WriteLine(failures == 0 ? "CONSOLE PROBE PASS" : "CONSOLE PROBE FAIL count=" + failures);
        return failures == 0 ? 0 : 1;
    }
}

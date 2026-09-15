using System.Diagnostics;
using System.Runtime;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;

// Behavioral tests of the selected rebuilt runtime, not a claim that every
// BCL/EH/codegen path is redirected. The observer-only probe remains separate.
internal static class RuntimeQualification
{
    [DllImport("__Internal", EntryPoint = "pal_qualification_foreign_thread")]
    private static extern unsafe int ForeignThread(delegate* unmanaged<nint, int> entry, nint context);
    [DllImport("__Internal", EntryPoint = "pal_qualification_fault_control")]
    private static extern uint FaultControl(uint count);
    [DllImport("__Internal", EntryPoint = "pal_qualification_fault_hits")]
    private static extern ulong FaultHits();
    [ThreadStatic] private static int threadToken;
    private static int finalized, callbacks;

    private sealed class Canary
    {
        private readonly byte[] payload = new byte[4096];
        public Canary() { payload[0] = 73; payload[^1] = 91; }
        ~Canary()
        {
            if (payload[0] != 73 || payload[^1] != 91) Environment.FailFast("finalizer payload corrupted");
            Interlocked.Increment(ref finalized);
        }
    }
    private static void Check(bool value, string message)
    {
        if (!value) throw new InvalidOperationException(message);
    }
    [MethodImpl(MethodImplOptions.NoInlining)]
    private static void MakeFinalizers(int count)
    {
        for (int i = 0; i < count; ++i) GC.KeepAlive(new Canary());
    }
    [MethodImpl(MethodImplOptions.NoInlining)]
    private static WeakReference MakeWeak() => new(new byte[8192]);
    [MethodImpl(MethodImplOptions.NoInlining)]
    private static int Churn(int count, int seed)
    {
        int sum = seed;
        var live = new object[32];
        for (int i = 0; i < count; ++i)
        {
            var bytes = new byte[4096 + (i & 1023)];
            Check(bytes[0] == 0 && bytes[^1] == 0, "new memory not zeroed");
            bytes[0] = (byte)(i + seed); bytes[^1] = 0x57;
            live[i & 31] = bytes;
            sum = unchecked(sum * 31 + bytes[0]);
        }
        foreach (object item in live) Check(((byte[])item)[^1] == 0x57, "root corruption");
        GC.KeepAlive(live);
        return sum;
    }
    private static bool Filter(Exception error, byte[] root)
    {
        GC.Collect(0, GCCollectionMode.Forced, true);
        Check(root[0] == 31, "root lost in exception filter");
        return error is ApplicationException;
    }
    [MethodImpl(MethodImplOptions.NoInlining)]
    private static void ExceptionsAndRoots()
    {
        byte[] root = new byte[4096]; root[0] = 31;
        int finallyCount = 0;
        try
        {
            try { throw new ApplicationException("qualification"); }
            catch (ApplicationException) { throw; }
            finally { ++finallyCount; }
        }
        catch (Exception error) when (Filter(error, root))
        {
            Check(error.StackTrace?.Contains(nameof(ExceptionsAndRoots)) == true, "unwind stack trace missing frame");
        }
        finally { ++finallyCount; }
        Check(finallyCount == 2 && root[0] == 31, "exception unwind/finally failure");
        GC.KeepAlive(root);
    }
    [UnmanagedCallersOnly]
    private static int ForeignEntry(nint context)
    {
        // An exception must never cross the native ABI.
        try
        {
            var root = (byte[])GCHandle.FromIntPtr(context).Target!;
            Check(root[0] == 0x35, "foreign thread root lost");
            Churn(256, 9); ExceptionsAndRoots();
            Interlocked.Increment(ref callbacks);
            return 0;
        }
        catch { return 1; }
    }
    private static unsafe void ForeignThreadRoundTrip()
    {
        byte[] root = new byte[16384]; root[0] = 0x35;
        GCHandle handle = GCHandle.Alloc(root);
        try { Check(ForeignThread(&ForeignEntry, GCHandle.ToIntPtr(handle)) == 0, "foreign thread callback failed"); }
        finally { handle.Free(); }
    }
    private static void Collect()
    {
        GC.Collect(GC.MaxGeneration, GCCollectionMode.Forced, true, true);
        GC.WaitForPendingFinalizers();
        GC.Collect(GC.MaxGeneration, GCCollectionMode.Forced, true, true);
    }
    private static void Stress(int seconds)
    {
        var root = new byte[65536]; root[0] = 12; root[^1] = 14;
        GCHandle pinned = GCHandle.Alloc(root, GCHandleType.Pinned);
        nint initialAddress = pinned.AddrOfPinnedObject();
        WeakReference weak = MakeWeak();
        var errors = new System.Collections.Concurrent.ConcurrentQueue<Exception>();
        using var start = new ManualResetEventSlim(false);
        using var stop = new CancellationTokenSource();
        object mutex = new();
        int rounds = 0;
        Thread[] workers = Enumerable.Range(1, 4).Select(id => new Thread(() =>
        {
            try
            {
                threadToken = id; start.Wait();
                while (!stop.IsCancellationRequested)
                {
                    Churn(128, id);
                    lock (mutex)
                    {
                        lock (mutex) Check(threadToken == id, "TLS/recursive monitor isolation failed");
                    }
                    MakeFinalizers(4); ExceptionsAndRoots();
                    Interlocked.Increment(ref rounds);
                }
            }
            catch (Exception error) { errors.Enqueue(error); stop.Cancel(); }
        })).ToArray();
        try
        {
            foreach (Thread worker in workers) worker.Start();
            start.Set();
            Stopwatch watch = Stopwatch.StartNew();
            do
            {
                ForeignThreadRoundTrip(); Collect();
                Check(root[0] == 12 && root[^1] == 14, "pinned payload corrupted");
                Check(pinned.AddrOfPinnedObject() == initialAddress, "pinned object moved");
            } while (watch.Elapsed.TotalSeconds < seconds && !stop.IsCancellationRequested);
        }
        finally
        {
            stop.Cancel(); start.Set();
            foreach (Thread worker in workers)
                if (worker.IsAlive) Check(worker.Join(TimeSpan.FromSeconds(30)), "worker termination timed out");
            pinned.Free();
        }
        Check(errors.IsEmpty, errors.TryPeek(out Exception? first) ? first.ToString() : "worker failed");
        Collect();
        Check(!weak.IsAlive, "unrooted object remained alive");
        Check(rounds > 0 && finalized == rounds * 4 && callbacks > 0, "stress accounting mismatch");
        Console.WriteLine($"STRESS PASS rounds={rounds} finalizers={finalized} callbacks={callbacks} pinned=true weak=true");
    }
    [MethodImpl(MethodImplOptions.NoInlining)]
    private static int ExhaustHeap()
    {
        var held = new byte[512][];
        int count = 0;
        try
        {
            for (; count < held.Length; ++count)
            {
                held[count] = new byte[1024 * 1024]; held[count][0] = (byte)count;
            }
        }
        catch (OutOfMemoryException)
        {
            for (int i = 0; i < count; ++i) Check(held[i][0] == (byte)i, "OOM corrupted a live allocation");
            Array.Clear(held);
            return count;
        }
        GC.KeepAlive(held);
        throw new InvalidOperationException("expected bounded heap exhaustion; verify the 128 MiB hard limit");
    }
    private static void OomRecovery()
    {
        var root = new byte[16384]; root[0] = 123;
        for (int wave = 0; wave < 3; ++wave)
        {
            int count = ExhaustHeap();
            Check(count > 0 && count < 512, "invalid exhaustion count");
            Collect();
            byte[] recovered = new byte[4 * 1024 * 1024];
            Check(recovered[0] == 0 && recovered[^1] == 0 && root[0] == 123, "OOM recovery failed");
            GC.KeepAlive(recovered);
            Console.WriteLine($"OOM RECOVERY PASS wave={wave} retained_mib={count}");
        }
        GC.KeepAlive(root);
    }
    private static void FaultRecovery()
    {
        // Only the fault-provider binary implements this test-only control.
        byte[] root = new byte[4096]; root[0] = 99;
        Check(FaultControl(1) == 0, "fault provider not linked");
        try
        {
            for (int wave = 0; wave < 8 && FaultHits() == 0; ++wave)
            {
                try { GC.KeepAlive(new byte[32 * 1024 * 1024]); }
                catch (OutOfMemoryException) { /* allowed: transient commit failure */ }
                Collect();
            }
        }
        finally { Check(FaultControl(0) == 0, "cannot disable injection"); }
        Check(FaultHits() == 1, "GC did not encounter exactly one injected commit failure");
        Collect();
        byte[] recovered = new byte[4 * 1024 * 1024];
        Check(recovered[0] == 0 && recovered[^1] == 0 && root[0] == 99, "fault recovery corrupted roots");
        GC.KeepAlive(root); GC.KeepAlive(recovered);
        Console.WriteLine("FAULT RECOVERY PASS injected_commit_failures=1");
    }
    internal static int Run(string[] args)
    {
        Check(args.Length == 3, "usage: qualify stress|oom|fault|benchmark workstation|server");
        Check(!RuntimeFeature.IsDynamicCodeSupported, "qualification requires NativeAOT");
        Check(args[2] is "workstation" or "server", "invalid GC profile");
        Check(GCSettings.IsServerGC == (args[2] == "server"), "actual GC mode differs from requested profile");
        Stopwatch watch = Stopwatch.StartNew();
        switch (args[1])
        {
            case "stress":
                int seconds = int.TryParse(Environment.GetEnvironmentVariable("PAL_STRESS_SECONDS"), out int s) ? s : 15;
                Check(seconds is >= 1 and <= 3600, "stress duration out of range");
                Stress(seconds); break;
            case "oom": OomRecovery(); break;
            case "fault": FaultRecovery(); break;
            case "benchmark":
                int checksum = 0;
                for (int i = 0; i < 16; ++i) checksum ^= Churn(4096, i);
                Console.WriteLine($"WORKLOAD checksum={checksum}"); break;
            default: throw new ArgumentException("unknown qualification mode");
        }
        Console.WriteLine($"QUALIFICATION PASS mode={args[1]} gc={args[2]} elapsed_ms={watch.Elapsed.TotalMilliseconds:F3} total_allocated={GC.GetTotalAllocatedBytes()} collections={GC.CollectionCount(2)}");
        return 0;
    }
}

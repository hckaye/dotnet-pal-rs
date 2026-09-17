using System;
using System.Diagnostics;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using System.Security.Cryptography;

// Managed workload for the browser port. It only imports statistics observers
// from the adapter; it never calls boundary operations directly, so any Rust
// activity it reports was driven by the runtime (GC storage, clocks).
public static class Program
{
    [StructLayout(LayoutKind.Sequential)]
    private struct StorageStats { public ulong Allocate, Zero, Release, Failed; }
    [StructLayout(LayoutKind.Sequential)]
    private struct AdapterStats { public ulong Reserve, Commit, Decommit, Release, Reset, Failed, Owned, Peak; }
    [StructLayout(LayoutKind.Sequential)]
    private struct ServicesStats { public ulong Clock, Sleep, Yield, Failed; }
    [DllImport("__Internal", EntryPoint = "dotnet_pal_linear_probe_stats")]
    private static extern uint Observe(out StorageStats storage, out AdapterStats adapter, out ServicesStats services);
    [DllImport("__Internal", EntryPoint = "dotnet_pal_linear_probe_capacity")]
    private static extern ulong Capacity();

    private static void Check(bool ok, string message) { if (!ok) throw new Exception(message); }

    [MethodImpl(MethodImplOptions.NoInlining)]
    private static long Wave(int arrays)
    {
        var live = new byte[arrays][];
        for (int i = 0; i < live.Length; ++i)
        {
            live[i] = new byte[65536];
            Check(live[i][0] == 0 && live[i][65535] == 0, "fresh storage was not zeroed");
            live[i][0] = (byte)i; live[i][65535] = 73;
        }
        GC.Collect();
        long checksum = 0;
        for (int i = 0; i < live.Length; ++i)
        {
            Check(live[i][0] == (byte)i && live[i][65535] == 73, "root corruption after collection");
            checksum += live[i][0];
        }
        GC.KeepAlive(live);
        return checksum;
    }

    private sealed class Finalizable { public static int Count; ~Finalizable() { Count++; } }

    private static int Exceptions()
    {
        int caught = 0;
        for (int i = 0; i < 8; ++i)
        {
            try { try { throw new InvalidOperationException("inner " + i); } finally { caught++; } }
            catch (InvalidOperationException e) when (e.Message.EndsWith(i.ToString())) { caught++; }
        }
        return caught;
    }

    public static int Main(string[] args)
    {
        try { return Run(args); }
        catch (Exception error)
        {
            Console.Error.WriteLine("MANAGED BROWSER FAIL: " + error.Message);
            return 1;
        }
    }

    private static int Run(string[] args)
    {
        Check(!RuntimeFeature.IsDynamicCodeSupported, "AOT required");
        Check(Observe(out var storageBefore, out var adapterBefore, out var servicesBefore) == 0, "observer unavailable");
        Check(adapterBefore.Reserve > 0 && storageBefore.Allocate > 0, "GC startup storage did not cross the Rust boundary");
        Console.WriteLine($"page capacity={Capacity()} args={string.Join(",", args)}");
        Console.WriteLine("environment PAL_BROWSER=" + (Environment.GetEnvironmentVariable("PAL_BROWSER") ?? "<absent>"));
        Check(Environment.GetEnvironmentVariable("PAL_BROWSER") == "page", "environment did not come from the page");

        var clock = Stopwatch.StartNew();
        long checksum = 0;
        for (int wave = 0; wave < 3; ++wave) checksum += Wave(96 + wave * 32);
        for (int i = 0; i < 64; ++i) new Finalizable();
        GC.Collect(); GC.WaitForPendingFinalizers(); GC.Collect();
        Check(Finalizable.Count == 64, "finalizers did not run: " + Finalizable.Count);
        int caught = Exceptions();
        Check(caught == 16, "exception handling count " + caught);
        var random = RandomNumberGenerator.GetBytes(64);
        int nonzero = 0; foreach (var b in random) nonzero |= b;
        Check(nonzero != 0, "entropy");
        var now = DateTime.UtcNow;
        Check(now.Year >= 2024, "wall clock " + now);
        var text = new string('ß', 40);
        Check(System.Text.Encoding.UTF8.GetByteCount(text) == 80, "UTF-8");
        Console.WriteLine($"waves checksum={checksum} finalizers={Finalizable.Count} exceptions={caught} utc={now:O} elapsed_ms={clock.ElapsedMilliseconds}");

        Check(Observe(out var storage, out var adapter, out var services) == 0, "observer unavailable");
        // The GC reserves its initial range at startup and then commits logically
        // inside it; further Rust allocations only happen when it grows beyond it.
        Check(storage.Allocate >= storageBefore.Allocate && storage.Failed == 0, "Rust storage failures");
        Check(adapter.Commit > adapterBefore.Commit, "no further logical commits during the workload");
        Check(services.Clock > servicesBefore.Clock, "runtime clock did not cross the Rust boundary");
        Console.WriteLine($"MANAGED BROWSER PASS rust_allocations={storage.Allocate} commits={adapter.Commit} decommits={adapter.Decommit} peak_owned={adapter.Peak} clock={services.Clock}");
        return 0;
    }
}

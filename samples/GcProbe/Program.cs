using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;

internal static class Program
{
    [StructLayout(LayoutKind.Sequential)]
    private struct Stats
    {
        public ulong Reserve, Commit, Decommit, Release, Reset, Failed;
    }
    [DllImport("__Internal", EntryPoint = "dotnet_pal_probe_stats")]
    private static extern uint ReadStats(out Stats stats, nuint size);

    private static Stats Snapshot()
    {
        if (ReadStats(out Stats stats, (nuint)Marshal.SizeOf<Stats>()) != 0)
            throw new InvalidOperationException("PAL unavailable");
        return stats;
    }
    private static void Require(bool ok, string message)
    {
        if (!ok) throw new InvalidOperationException(message);
    }
    [MethodImpl(MethodImplOptions.NoInlining)]
    private static void AllocateWave()
    {
        var live = new byte[96][];
        for (int i = 0; i < live.Length; i++)
        {
            live[i] = new byte[1024 * 1024];
            Require(live[i][0] == 0 && live[i][^1] == 0, "new array is not zeroed");
            live[i][0] = (byte)i;
            live[i][^1] = 123;
        }
        for (int i = 0; i < live.Length; i++)
            Require(live[i][0] == (byte)i && live[i][^1] == 123, "live data corrupted");
        GC.KeepAlive(live);
    }
    private static void SmallAllocations()
    {
        for (int i = 0; i < 5000; i++)
        {
            var data = new object[] { i, new byte[2048], new string('x', 200) };
            GC.KeepAlive(data);
        }
    }
    public static int Main(string[] args)
    {
        bool wrapped = args.Length == 1 && args[0] == "wrapped";
        Require(wrapped || (args.Length == 1 && args[0] == "baseline"), "expected wrapped or baseline");
        Require(!RuntimeFeature.IsDynamicCodeSupported, "must run the published NativeAOT executable");
        Stats before = Snapshot();
        for (int wave = 0; wave < 3; wave++)
        {
            AllocateWave();
            GC.Collect(GC.MaxGeneration, GCCollectionMode.Forced, blocking: true, compacting: true);
            GC.WaitForPendingFinalizers();
        }
        Task.WaitAll(Enumerable.Range(0, 4).Select(_ => Task.Run(SmallAllocations)).ToArray());
        Stats after = Snapshot();
        if (wrapped)
        {
            Require(before.Reserve > 0, "GC startup did not reserve through the Rust boundary");
            Require(after.Commit > before.Commit, "allocations did not commit through Rust");
            Require(after.Failed == 0, "PAL reported a failed operation");
        }
        else
        {
            Require(after.Reserve == 0 && after.Commit == 0 && after.Decommit == 0 &&
                after.Release == 0 && after.Reset == 0 && after.Failed == 0,
                "negative control unexpectedly used the Rust VM boundary");
        }
        Console.WriteLine($"GC PROBE PASS mode={(wrapped ? "wrapped" : "baseline")} " +
            $"reserve={after.Reserve} commit_before={before.Commit} commit_after={after.Commit} " +
            $"decommit={after.Decommit} release={after.Release} reset={after.Reset}");
        return 0;
    }
}

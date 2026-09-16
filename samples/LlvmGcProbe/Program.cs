using System;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
public static class Program
{
    [StructLayout(LayoutKind.Sequential)]
    private struct StorageStats { public ulong Allocate, Zero, Release, Failed; }
    [StructLayout(LayoutKind.Sequential)]
    private struct AdapterStats { public ulong Reserve, Commit, Decommit, Release, Reset, Failed, Owned, Peak; }
    [StructLayout(LayoutKind.Sequential)]
    private struct ServicesStats { public ulong Clock, Sleep, Yield, Failed; }
    [DllImport("__Internal", EntryPoint="dotnet_pal_linear_probe_stats")]
    private static extern uint Observe(out StorageStats storage, out AdapterStats adapter, out ServicesStats services);
    [DllImport("__Internal", EntryPoint="pal_p1_error_text_test")]
    private static extern int CheckErrorText();
    private static void Check(bool ok, string message) { if (!ok) throw new Exception(message); }
    [MethodImpl(MethodImplOptions.NoInlining)]
    private static long Wave()
    {
        var live = new byte[128][];
        for (int i = 0; i < live.Length; ++i)
        {
            live[i] = new byte[65536];
            Check(live[i][0] == 0 && live[i][65535] == 0, "not zeroed");
            live[i][0] = (byte)i; live[i][65535] = 73;
        }
        GC.Collect();
        long checksum = 0;
        for (int i = 0; i < live.Length; ++i)
        {
            Check(live[i][0] == (byte)i && live[i][65535] == 73, "root corruption");
            checksum += live[i][0];
        }
        GC.KeepAlive(live);
        return checksum;
    }
    public static int Main(string[] args)
    {
        try { return Run(args); }
        catch (Exception error)
        {
            // Report only the managed message: the experimental runtime's fatal
            // stack-trace formatter must not obscure a failed assertion.
            Console.Error.WriteLine("MANAGED WASI FAIL: " + error.Message);
            return 1;
        }
    }
    private static int Run(string[] args)
    {
        Check(args.Length == 1 && (args[0] == "baseline" || args[0] == "wrapped" || args[0] == "source"), "expected probe mode");
        bool wrapped = args[0] != "baseline";
        Check(!RuntimeFeature.IsDynamicCodeSupported, "AOT required");
        Check(CheckErrorText() == 1, "error conversion failed");
        Check(Observe(out StorageStats storageBefore, out AdapterStats before, out ServicesStats servicesBefore) == 0, "observer failed");
        // The initial observer precedes the workload: startup evidence is not
        // manufactured by later allocation or finalization tests.
        Console.WriteLine("WASM QUALIFICATION BEGIN mode=" + args[0]);
        WasmQualification.Run();
        Check(Observe(out StorageStats exhaustedStorage, out AdapterStats exhausted, out ServicesStats exhaustedServices) == 0, "post-OOM observer failed");
        if (wrapped)
        {
            Check(exhaustedStorage.Failed > storageBefore.Failed, "OOM never reached the real Rust arena");
            Check(exhausted.Failed - before.Failed == exhaustedStorage.Failed - storageBefore.Failed,
                "non-allocation adapter failures during OOM qualification");
        }
        long checksum = 0;
        int caught = 0, finals = 0;
        for (int wave = 0; wave < 8; ++wave)
        {
            checksum += Wave();
            GC.Collect();
            try { throw new ApplicationException("EH probe"); }
            catch (ApplicationException) { ++caught; }
            finally { ++finals; }
        }
        Check(caught == 8 && finals == 8 && checksum == 65024, "accounting");
        Check(Observe(out StorageStats storageAfter, out AdapterStats after, out ServicesStats servicesAfter) == 0, "observer failed");
        if (wrapped)
        {
            Check(before.Reserve > 0 && storageBefore.Allocate > 0, "GC startup did not use Rust linear storage");
            Check(after.Commit > before.Commit, "managed allocations did not cross the linear adapter");
            Check(servicesAfter.Clock > servicesBefore.Clock, "GC clock did not cross Rust");
            Check(storageAfter.Failed == exhaustedStorage.Failed && after.Failed == exhausted.Failed && servicesAfter.Failed == 0, "unexpected boundary failure");
        }
        else
        {
            Check(storageAfter.Allocate == 0 && storageAfter.Zero == 0 && storageAfter.Release == 0 && storageAfter.Failed == 0 &&
                  after.Reserve == 0 && after.Commit == 0 && after.Failed == 0 && servicesAfter.Clock == 0,
                  "negative control unexpectedly used the Rust boundary");
        }
        Console.WriteLine("MANAGED WASI PASS mode=" + args[0] + " checksum=" + checksum + " catches=" + caught +
            " rust_allocations=" + storageAfter.Allocate + " commits_before=" + before.Commit + " commits_after=" + after.Commit +
            " decommits=" + after.Decommit + " peak_owned=" + after.Peak + " clock=" + servicesAfter.Clock);
        return 0;
    }
}

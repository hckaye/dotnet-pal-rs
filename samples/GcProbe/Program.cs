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
    [StructLayout(LayoutKind.Sequential)]
    private struct ServicesStats
    {
        public ulong Clock, Sleep, Yield, Failed;
    }
    [DllImport("__Internal", EntryPoint = "dotnet_pal_probe_services_stats")]
    private static extern uint ReadServicesStats(out ServicesStats stats, nuint size);
    private static ServicesStats ServicesSnapshot()
    {
        if (ReadServicesStats(out ServicesStats stats, (nuint)Marshal.SizeOf<ServicesStats>()) != 0)
            throw new InvalidOperationException("services observer unavailable");
        return stats;
    }
    [StructLayout(LayoutKind.Sequential)]
    private struct KernelStats
    {
        public ulong EventCreate, EventWait, EventTimeout, EventSet, MutexCreate, MutexLock;
        public ulong ThreadCreate, TlsCreate, TlsSet, StackBounds, Barrier, Failed;
    }
    [DllImport("__Internal", EntryPoint = "dotnet_pal_probe_kernel_stats")]
    private static extern uint ReadKernelStats(out KernelStats stats, nuint size);
    private static KernelStats KernelSnapshot()
    {
        if (ReadKernelStats(out KernelStats stats, (nuint)Marshal.SizeOf<KernelStats>()) != 0)
            throw new InvalidOperationException("kernel observer unavailable");
        return stats;
    }
    [StructLayout(LayoutKind.Sequential)]
    private struct ContextStats { public ulong Installs, Restores, Requests, Unblocks, Threads, Rejected; }
    [StructLayout(LayoutKind.Sequential)]
    private struct RuntimeStats
    {
        public ulong Environment, Identity, Realtime, Entropy, MappingAllocate, MappingRelease, MappingProtect;
        public ulong ModuleOpen, ModuleSymbol, ModuleClose, ModuleInfo, Failed;
    }
    [DllImport("__Internal",EntryPoint="dotnet_pal_probe_context_stats")]
    private static extern uint ReadContextStats(out ContextStats stats,nuint size);
    [DllImport("__Internal",EntryPoint="dotnet_pal_probe_runtime_stats")]
    private static extern uint ReadRuntimeStats(out RuntimeStats stats,nuint size);
    private static int stopBusy;
    private static long busyChecksum;
    [MethodImpl(MethodImplOptions.NoInlining)]
    private static void BusyManagedLoop(ManualResetEventSlim ready)
    {
        long checksum=17;
        ready.Set();
        // No allocations or calls after ready: collections must coordinate with
        // a running managed thread, not just wait for an allocation slow path.
        while (Volatile.Read(ref stopBusy)==0)
            checksum=unchecked((checksum*6364136223846793005L)^0x12345678);
        Interlocked.Exchange(ref busyChecksum,checksum);
    }
    private static void ActivationAndRuntimeProbe(bool routed)
    {
        Require(ReadContextStats(out ContextStats before,48)==0,"context observer unavailable");
        using var ready=new ManualResetEventSlim(false);
        stopBusy=0;
        var worker=new Thread(()=>BusyManagedLoop(ready));worker.Start();
        Require(ready.Wait(TimeSpan.FromSeconds(10)),"busy thread did not start");
        try
        {
            for(int i=0;i<16;++i)
                GC.Collect(GC.MaxGeneration,GCCollectionMode.Forced,blocking:true,compacting:true);
        }
        finally
        {
            Volatile.Write(ref stopBusy,1);
            Require(worker.Join(TimeSpan.FromSeconds(10)),"busy thread did not resume");
        }
        Require(ReadContextStats(out ContextStats after,48)==0,"context observer unavailable");
        Require(ReadRuntimeStats(out RuntimeStats runtime,96)==0,"runtime observer unavailable");
        if(routed)
        {
            Require(after.Installs>=2 && after.Requests>before.Requests && after.Unblocks>0 && after.Threads>0,
                "actual GC activation did not use the native context capability");
            Require(runtime.Identity>0 && runtime.ModuleInfo>0,"runtime identity/module discovery bypassed PAL");
        }
        else
        {
            Require(after.Installs==0 && after.Requests==0 && after.Unblocks==0 && after.Threads==0,
                "negative control unexpectedly invoked native context operations");
            Require(runtime.Identity==0 && runtime.ModuleInfo==0,"negative control unexpectedly invoked runtime operations");
        }
        Console.WriteLine($"NATIVE CONTEXT GC PASS routed={routed} installs={after.Installs} requests_before={before.Requests} requests_after={after.Requests} threads={after.Threads} identity={runtime.Identity} modules={runtime.ModuleInfo} checksum={busyChecksum}");
    }
    private static int finalized;
    private sealed class FinalizerCanary
    {
        ~FinalizerCanary() { Interlocked.Increment(ref finalized); }
    }
    [MethodImpl(MethodImplOptions.NoInlining)]
    private static void CreateFinalizers()
    {
        for (int i = 0; i < 128; ++i) GC.KeepAlive(new FinalizerCanary());
    }
    private static void ThreadAndFinalizerWork()
    {
        CreateFinalizers();
        var workers = Enumerable.Range(0, 8).Select(_ => new Thread(SmallAllocations)).ToArray();
        foreach (Thread worker in workers) worker.Start();
        foreach (Thread worker in workers) Require(worker.Join(TimeSpan.FromSeconds(30)), "native thread failed to terminate");
        GC.Collect(GC.MaxGeneration, GCCollectionMode.Forced, blocking: true, compacting: true);
        GC.WaitForPendingFinalizers();
        GC.Collect(GC.MaxGeneration, GCCollectionMode.Forced, blocking: true, compacting: true);
        Require(Volatile.Read(ref finalized) == 128, "finalizer execution was lost or repeated");
    }
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
#if PAL_QUALIFICATION
        if (args.Length > 0 && args[0] == "qualify") return RuntimeQualification.Run(args);
#endif
        bool sourceKernel = args.Length == 1 && args[0] == "source-kernel";
        bool sourceServices = sourceKernel || (args.Length == 1 && args[0] == "source-services");
        bool wrapped = sourceServices || (args.Length == 1 && args[0] == "wrapped");
        Require(wrapped || (args.Length == 1 && args[0] == "baseline"), "expected wrapped, baseline or source-services");
        Require(!RuntimeFeature.IsDynamicCodeSupported, "must run the published NativeAOT executable");
        Stats before = Snapshot();
        ServicesStats servicesBefore = ServicesSnapshot();
        KernelStats kernelBefore = KernelSnapshot();
        ActivationAndRuntimeProbe(sourceKernel);
        for (int wave = 0; wave < 3; wave++)
        {
            AllocateWave();
            GC.Collect(GC.MaxGeneration, GCCollectionMode.Forced, blocking: true, compacting: true);
            GC.WaitForPendingFinalizers();
        }
        Task.WaitAll(Enumerable.Range(0, 4).Select(_ => Task.Run(SmallAllocations)).ToArray());
        ThreadAndFinalizerWork();
        SupportProbe.Check(sourceKernel);
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
        ServicesStats servicesAfter = ServicesSnapshot();
        if (sourceServices)
        {
            Require(servicesAfter.Clock > servicesBefore.Clock, "GC clock did not pass through Rust");
            Require(servicesAfter.Failed == 0, "GC service failure");
        }
        else
        {
            Require(servicesAfter.Clock == 0 && servicesAfter.Sleep == 0 && servicesAfter.Yield == 0 &&
                servicesAfter.Failed == 0, "service negative control unexpectedly crossed Rust");
        }
        KernelStats kernelAfter = KernelSnapshot();
        if (sourceKernel)
        {
            Require(kernelAfter.EventCreate > 0 && kernelAfter.EventWait > kernelBefore.EventWait &&
                kernelAfter.EventSet > kernelBefore.EventSet, "runtime event path did not cross Rust");
            Require(kernelAfter.MutexLock > kernelBefore.MutexLock, "runtime/GC locks did not cross Rust");
            Require(kernelAfter.ThreadCreate > 0, "background/finalizer threads did not cross Rust");
            Require(kernelAfter.TlsCreate > 0 && kernelAfter.TlsSet > kernelBefore.TlsSet,
                "thread attachment/termination TLS did not cross Rust");
            Require(kernelAfter.StackBounds > kernelBefore.StackBounds && kernelAfter.Barrier > kernelBefore.Barrier,
                "stack bounds or process barriers did not cross Rust");
            Require(kernelAfter.Failed == 0, "unexpected kernel boundary failure");
        }
        else
        {
            Require(kernelAfter.EventCreate == 0 && kernelAfter.EventWait == 0 && kernelAfter.EventSet == 0 &&
                kernelAfter.MutexCreate == 0 && kernelAfter.MutexLock == 0 && kernelAfter.ThreadCreate == 0 &&
                kernelAfter.TlsCreate == 0 && kernelAfter.TlsSet == 0 && kernelAfter.StackBounds == 0 &&
                kernelAfter.Barrier == 0 && kernelAfter.Failed == 0, "kernel negative control crossed Rust");
        }
        Console.WriteLine($"KERNEL GC PROBE PASS source={sourceKernel} events={kernelAfter.EventCreate} " +
            $"waits={kernelAfter.EventWait} locks={kernelAfter.MutexLock} threads={kernelAfter.ThreadCreate} " +
            $"tls={kernelAfter.TlsSet} stacks={kernelAfter.StackBounds} barriers={kernelAfter.Barrier} finalizers={finalized}");
        Console.WriteLine($"SERVICES PROBE PASS source={sourceServices} clock_before={servicesBefore.Clock} " +
            $"clock_after={servicesAfter.Clock} sleep={servicesAfter.Sleep} yield={servicesAfter.Yield}");
        Console.WriteLine($"GC PROBE PASS mode={(wrapped ? "wrapped" : "baseline")} " +
            $"reserve={after.Reserve} commit_before={before.Commit} commit_after={after.Commit} " +
            $"decommit={after.Decommit} release={after.Release} reset={after.Reset}");
        return 0;
    }
}

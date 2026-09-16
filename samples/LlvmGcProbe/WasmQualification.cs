using System;
using System.Runtime;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;

// Behavioral qualification of the explicitly single-threaded, bounded WASIp1
// profile. Finalization is explicitly pumped; no background thread is implied.
internal static class WasmQualification
{
    [DllImport("__Internal", EntryPoint="dotnet_pal_linear_probe_capacity")]
    private static extern ulong ArenaCapacity();
    private static int finalized;
    private sealed class Canary
    {
        private readonly byte[] bytes = new byte[4096];
        internal Canary() { bytes[0] = 0x31; bytes[4095] = 0x79; }
        ~Canary()
        {
            Check(bytes[0] == 0x31 && bytes[4095] == 0x79, "finalizer payload corrupted");
            // The pinned single-threaded runtime explicitly treats reentrant
            // finalizer waits as a no-op rather than deadlocking itself.
            GC.WaitForPendingFinalizers();
            ++finalized;
        }
    }
    private static void Check(bool ok, string message)
    {
        if (!ok) throw new InvalidOperationException(message);
    }
    [MethodImpl(MethodImplOptions.NoInlining)]
    private static void MakeFinalizers()
    {
        for (int i = 0; i < 64; ++i) GC.KeepAlive(new Canary());
    }
    [MethodImpl(MethodImplOptions.NoInlining)]
    private static WeakReference MakeWeak() => new WeakReference(new byte[8192]);
    private static void Collect()
    {
        GC.Collect(GC.MaxGeneration, GCCollectionMode.Forced, true, true);
        GC.WaitForPendingFinalizers();
        GC.Collect(GC.MaxGeneration, GCCollectionMode.Forced, true, true);
    }
    [MethodImpl(MethodImplOptions.NoInlining)]
    private static int Exhaust()
    {
        byte[][] held = new byte[256][];
        int count = 0;
        try
        {
            for (; count < held.Length; ++count)
            {
                held[count] = new byte[1024 * 1024];
                Check(held[count][0] == 0 && held[count][1048575] == 0, "new OOM wave memory not zeroed");
                held[count][0] = (byte)count; held[count][1048575] = 0x51;
            }
        }
        catch (OutOfMemoryException)
        {
            for (int i = 0; i < count; ++i)
                Check(held[i][0] == (byte)i && held[i][1048575] == 0x51, "OOM damaged live storage");
            Array.Clear(held);
            return count;
        }
        GC.KeepAlive(held);
        throw new InvalidOperationException("expected bounded Wasm heap exhaustion");
    }
    internal static void Run()
    {
        Check(!GCSettings.IsServerGC, "Wasm profile must be single-threaded Workstation GC");
        // GCHeapHardLimit is a 64-bit-only setting. Do not pretend that the
        // wasm32 GC memory-information value enforces a managed heap limit.
        // The runner separately audits and enforces the 128 MiB module maximum.
        ulong cap = ArenaCapacity();
        Check(cap == 64UL * 1024 * 1024, "actual Rust arena capacity is not 64 MiB");
        Console.WriteLine("WASM ARENA PREFLIGHT PASS bytes=" + cap);
        byte[] root = new byte[65536]; root[0] = 43; root[65535] = 72;
        GCHandle pinned = GCHandle.Alloc(root, GCHandleType.Pinned);
        IntPtr initial = pinned.AddrOfPinnedObject();
        WeakReference weak = MakeWeak();
        try
        {
            MakeFinalizers();
            Collect();
            Check(finalized == 64, "explicit Wasm finalization accounting mismatch: " + finalized);
            Check(!weak.IsAlive, "unrooted weak target remained alive");
            for (int wave = 0; wave < 3; ++wave)
            {
                int count = Exhaust();
                Check(count > 0 && count < 64, "unexpected number of retained MiB: " + count);
                Collect();
                byte[] recovered = new byte[2 * 1024 * 1024];
                Check(recovered[0] == 0 && recovered[recovered.Length - 1] == 0, "OOM recovery not zeroed");
                Check(root[0] == 43 && root[65535] == 72 && pinned.AddrOfPinnedObject() == initial,
                    "pinned object moved or was corrupted");
                GC.KeepAlive(recovered);
                Console.WriteLine("WASM OOM RECOVERY PASS wave=" + wave + " retained_mib=" + count);
            }
        }
        finally { pinned.Free(); }
        GC.KeepAlive(root);
        Console.WriteLine("WASM ROOTS AND FINALIZERS PASS finalizers=" + finalized + " weak=true pinned=true reentrant_wait=true");
    }
}

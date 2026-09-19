using System.Runtime.InteropServices;
// CPU counts, affinity and memory figures reach the runtime through the topology group when the runtime was built from
// source against the boundary, and not at all when the published runtime asks the OS itself.
internal static class TopologyProbe
{
    [StructLayout(LayoutKind.Sequential)]
    private struct Stats { public ulong Cpu, Affinity, Memory, Cache, Features, Rejected; }
    [DllImport("__Internal", EntryPoint="dotnet_pal_probe_topology_stats")]
    private static extern uint Read(out Stats stats, nuint size);
    internal static void Check(bool routed)
    {
        if (Read(out Stats s, (nuint)Marshal.SizeOf<Stats>()) != 0)
            throw new InvalidOperationException("topology observer unavailable");
        if (routed ? s.Cpu == 0 || s.Memory == 0 : s.Cpu != 0 || s.Affinity != 0 || s.Memory != 0 || s.Cache != 0)
            throw new InvalidOperationException("runtime topology calls did not match the selected control");
        Console.WriteLine($"TOPOLOGY GC PASS routed={routed} cpu={s.Cpu} affinity={s.Affinity} memory={s.Memory} cache={s.Cache}");
    }
}

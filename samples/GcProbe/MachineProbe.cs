using System.Runtime.InteropServices;
internal static class MachineProbe
{
    [StructLayout(LayoutKind.Sequential)]
    private struct Stats { public ulong Query, Affinity, Bind, Current, Rejected; }
    [DllImport("__Internal", EntryPoint="dotnet_pal_probe_machine_stats")]
    private static extern uint Read(out Stats stats, nuint size);
    internal static void Check(bool routed)
    {
        if (Read(out Stats s, (nuint)Marshal.SizeOf<Stats>()) != 0)
            throw new InvalidOperationException("machine observer unavailable");
        if (routed ? s.Query == 0 || s.Affinity == 0 : s.Query != 0 || s.Affinity != 0 || s.Bind != 0 || s.Current != 0)
            throw new InvalidOperationException("runtime machine calls did not match the selected control");
        Console.WriteLine($"MACHINE GC PASS routed={routed} query={s.Query} affinity={s.Affinity} bind={s.Bind} current={s.Current}");
    }
}

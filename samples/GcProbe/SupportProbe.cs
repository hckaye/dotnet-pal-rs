using System.Runtime.InteropServices;
internal static class SupportProbe
{
    [StructLayout(LayoutKind.Sequential)]
    private struct Stats
    {
        public ulong Allocate, Resize, Release, Create, Read, Write, Unlock, Destroy, Diagnostic, Name, Failed;
    }
    [DllImport("__Internal", EntryPoint="dotnet_pal_probe_support_stats")]
    private static extern uint ReadStats(out Stats stats, nuint size);
    [StructLayout(LayoutKind.Sequential)]
    private struct ImageStats { public ulong Iterate, Seen, Address, Failed; }
    [DllImport("__Internal", EntryPoint="dotnet_pal_probe_image_stats")]
    private static extern uint ReadImages(out ImageStats stats, nuint size);
    public static void Check(bool routed)
    {
        if (ReadStats(out Stats s,88)!=0)throw new InvalidOperationException("support observer unavailable");
        if (routed)
        {
            if (s.Name==0 || s.Failed!=0)throw new InvalidOperationException("runtime support path failed/bypassed");
            if (Environment.GetEnvironmentVariable("PAL_EXPECT_NATIVE_HEAP")=="1" && s.Allocate<2)
                throw new InvalidOperationException("startup dump formatting did not use the native helper heap");
        }
        else if (s.Allocate!=0 || s.Resize!=0 || s.Release!=0 || s.Create!=0 || s.Read!=0 || s.Write!=0 || s.Unlock!=0 || s.Destroy!=0 || s.Diagnostic!=0 || s.Name!=0 || s.Failed!=0)
            throw new InvalidOperationException("support negative control crossed Rust");
        // Exercise actual runtime stack discovery; imports remain observers only.
        GC.KeepAlive(Environment.StackTrace);
        if(ReadImages(out ImageStats images,32)!=0)throw new InvalidOperationException("image observer unavailable");
        if(routed) {
            if(images.Iterate==0 || images.Seen==0)throw new InvalidOperationException("native image discovery bypassed Rust");
            if(Environment.GetEnvironmentVariable("PAL_EXPECT_NATIVE_HEAP")=="1" && images.Address==0)
                throw new InvalidOperationException("dump path symbolization bypassed Rust");
        } else if(images.Iterate!=0 || images.Seen!=0 || images.Address!=0 || images.Failed!=0)
            throw new InvalidOperationException("image negative control crossed Rust");
        Console.WriteLine($"NATIVE IMAGE RUNTIME PASS routed={routed} iterations={images.Iterate} seen={images.Seen} symbols={images.Address}");
        Console.WriteLine($"NATIVE SUPPORT RUNTIME PASS routed={routed} allocate={s.Allocate} locks={s.Create} reads={s.Read} writes={s.Write} names={s.Name} diagnostics={s.Diagnostic}");
    }
}

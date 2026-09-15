#if PAL_QUALIFICATION
using System.Globalization;
using System.Runtime.CompilerServices;

// Test-only startup preflight. A passing OOM test requires the requested cap to
// actually be active; silently ignored environment configuration is a test error.
internal static class HeapLimitPreflight
{
    [ModuleInitializer]
    internal static void Initialize()
    {
        string[] arguments = Environment.GetCommandLineArgs();
        if (arguments.Length < 2 || arguments[1] != "qualify") return;
        string? text = Environment.GetEnvironmentVariable("DOTNET_GCHeapHardLimit");
        if (text is null || !ulong.TryParse(text, NumberStyles.AllowHexSpecifier,
            CultureInfo.InvariantCulture, out ulong expected) || expected != 128UL * 1024 * 1024)
            throw new InvalidOperationException("qualification requires the audited 128 MiB hex-only heap cap");
        long actual = GC.GetGCMemoryInfo().TotalAvailableMemoryBytes;
        if (actual <= 0 || (ulong)actual != expected)
            throw new InvalidOperationException($"heap cap not active: expected={expected}, actual={actual}");
        Console.WriteLine($"HEAP LIMIT PREFLIGHT PASS bytes={actual}");
    }
}
#endif

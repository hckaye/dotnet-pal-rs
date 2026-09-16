using System;
using System.IO;
using System.Text;
using System.Security.Cryptography;

// Exercise the actual trimmed BCL, not manually-issued PAL calls. The host grants
// only a fresh temporary preopened directory to this test and deletes it afterward.
internal static class BclQualification
{
    private static void Check(bool value, string message)
    {
        if (!value) throw new InvalidOperationException(message);
    }
    internal static void Run()
    {
        Check(Environment.GetEnvironmentVariable("PAL_BCL_CANARY") == "real-wasi-environment", "BCL environment bypass or corruption");
        Check(Environment.GetEnvironmentVariable("PAL_BCL_MISSING") == null, "BCL missing environment semantics");
        DateTime now = DateTime.UtcNow;
        Check(now > new DateTime(2025, 1, 1, 0, 0, 0, DateTimeKind.Utc), "BCL realtime invalid");
        byte[] first = new byte[64], second = new byte[64];
        RandomNumberGenerator.Fill(first); RandomNumberGenerator.Fill(second);
        Check(!first.AsSpan().SequenceEqual(second), "BCL entropy repeats");
        string root = Environment.GetEnvironmentVariable("PAL_WORKSPACE") ?? throw new InvalidOperationException("missing workspace grant");
        string directory = Path.Combine(root, "bcl-validation");
        Directory.CreateDirectory(directory);
        string original = Path.Combine(directory, "original.txt"), moved = Path.Combine(directory, "moved.txt");
        const string payload = "NativeAOT / Rust boundary / 日本語 / \u0000 / \u03bb";
        File.WriteAllText(original, payload, Encoding.UTF8);
        Check(File.ReadAllText(original, Encoding.UTF8) == payload, "BCL UTF8 roundtrip");
        using (var stream = new FileStream(original, FileMode.Open, FileAccess.ReadWrite, FileShare.None))
        {
            long length = stream.Length;
            Check(length > payload.Length && stream.CanSeek, "BCL stream metadata");
            stream.Seek(0, SeekOrigin.End); stream.WriteByte(0x5a); stream.Flush(true);
            Check(stream.Length == length + 1, "BCL stream length");
            stream.Seek(-1, SeekOrigin.End); Check(stream.ReadByte() == 0x5a, "BCL seek/read");
            stream.SetLength(length);
        }
        File.Move(original, moved, overwrite: true);
        Check(!File.Exists(original) && File.Exists(moved), "BCL move/existence");
        Check(Directory.GetFiles(directory).Length == 1, "BCL directory enumeration");
        Check(File.ReadAllText(moved, Encoding.UTF8) == payload, "BCL rename changed data");
        bool missing = false;
        try { File.ReadAllBytes(Path.Combine(directory,"missing")); }
        catch (FileNotFoundException) { missing = true; }
        Check(missing, "BCL missing file exception");
        File.Delete(moved); Directory.Delete(directory);
        Check(!Directory.Exists(directory), "BCL cleanup");
        Console.WriteLine("MANAGED BCL PASS environment/realtime/entropy/UTF8/files/seek/truncate/rename/enumeration/errors");
    }
}

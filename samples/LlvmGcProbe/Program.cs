using System;
using System.Runtime.CompilerServices;
public static class Program
{
    public static int Main()
    {
        if (RuntimeFeature.IsDynamicCodeSupported) throw new Exception("AOT required");
        long checksum = 0;
        int caught = 0;
        for (int wave = 0; wave < 8; ++wave)
        {
            var live = new byte[64][];
            for (int i = 0; i < live.Length; ++i)
            {
                live[i] = new byte[8192];
                if (live[i][0] != 0 || live[i][8191] != 0) throw new Exception("not zeroed");
                live[i][0] = (byte)i; live[i][8191] = 73;
            }
            GC.Collect();
            for (int i = 0; i < live.Length; ++i)
            {
                if (live[i][0] != (byte)i || live[i][8191] != 73) throw new Exception("root corruption");
                checksum += live[i][0];
            }
            GC.KeepAlive(live);
            try { throw new ApplicationException("EH probe"); }
            catch (ApplicationException) { ++caught; }
        }
        if (caught != 8 || checksum != 16128) throw new Exception("accounting");
        Console.WriteLine("MANAGED WASI BASELINE PASS checksum=" + checksum + " catches=" + caught);
        return 0;
    }
}

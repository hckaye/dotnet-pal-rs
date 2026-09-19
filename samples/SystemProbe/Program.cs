using System.ComponentModel;
using System.Diagnostics;
using System.Runtime.InteropServices;

// Every check is an observable claim about the BCL running on the boundary's system, processes,
// notifications and files groups. What the port under test does not provide must fail the way
// the BCL documents.
static unsafe class Program
{
    private static int s_failures;
    private static void Check(bool ok, string what) { if (!ok) { s_failures++; Console.Error.WriteLine("FAIL " + what); } }
    private static bool Throws<T>(Action action) where T : Exception
    {
        try { action(); return false; }
        catch (T) { return true; }
        catch (Exception e) { Console.Error.WriteLine("unexpected " + e.GetType().Name + ": " + e.Message); return false; }
    }

    // ---- the process, the machine, the user --------------------------------------------------
    private static void SystemFacts()
    {
        var variables = Environment.GetEnvironmentVariables();
        string description = RuntimeInformation.OSDescription;
#if EXPECT_SYSTEM
        // The script that runs the probe sets this variable; the enumeration must contain it and agree with the lookup.
        Check((string?)variables["SYSTEM_PROBE_VALUE"] == "from-the-boundary", "enumerated environment lacks the probe variable");
        Check(Environment.GetEnvironmentVariable("SYSTEM_PROBE_VALUE") == "from-the-boundary", "environment lookup");
        Check(variables.Count >= 2, "environment count " + variables.Count);
        // On Linux the BCL prefers the distribution name from /etc/os-release, which it reads through the files group; the kernel's own
        // release reaches Environment.OSVersion through the system group.
        Check(description.Length > 0 && Environment.OSVersion.Version.Major >= 2, "OS description '" + description + "' version " + Environment.OSVersion.Version);
        Check(Environment.UserName.Length > 0, "user name is empty");
        Check(Environment.GetFolderPath(Environment.SpecialFolder.UserProfile).StartsWith('/'), "home directory");
        Check(Path.GetFileName(Environment.ProcessPath ?? "") == "SystemProbe", "process path '" + Environment.ProcessPath + "'");
        Console.WriteLine("system variables=" + variables.Count + " user=" + Environment.UserName + " os=" + Environment.OSVersion.VersionString);
#elif EXPECT_BAREMETAL
        // A machine without an environment or a user: the enumeration is empty and the texts are the port's own.
        Check(variables.Count == 0, "environment count " + variables.Count);
        Check(description.StartsWith("baremetal-aarch64 ", StringComparison.Ordinal), "OS description '" + description + "'");
        Console.WriteLine("system os=" + description);
#else
        Check(variables.Count == 0, "an environment was enumerated without the system group");
        Console.WriteLine("system facts absent, as expected");
#endif
        Check(RuntimeInformation.OSArchitecture == RuntimeInformation.ProcessArchitecture, "architecture");
    }

    // ---- optional file operations ------------------------------------------------------------------
    private static void Links()
    {
        string root = Path.Combine(Path.GetTempPath(), "system-probe-" + Environment.ProcessId);
        if (Directory.Exists(root)) Directory.Delete(root, recursive: true);
        Directory.CreateDirectory(Path.Combine(root, "inner"));
        string file = Path.Combine(root, "inner", "data.txt");
        File.WriteAllText(file, "content");
#if EXPECT_LINKS
        // Permission bits and timestamps.
        File.SetUnixFileMode(file, UnixFileMode.UserRead | UnixFileMode.UserWrite | UnixFileMode.GroupRead);
        Check(File.GetUnixFileMode(file) == (UnixFileMode.UserRead | UnixFileMode.UserWrite | UnixFileMode.GroupRead), "mode " + File.GetUnixFileMode(file));
        var stamp = new DateTime(2021, 3, 4, 5, 6, 7, DateTimeKind.Utc);
        File.SetLastWriteTimeUtc(file, stamp);
        Check(File.GetLastWriteTimeUtc(file) == stamp, "write time " + File.GetLastWriteTimeUtc(file).ToString("O"));
        File.SetLastAccessTimeUtc(file, stamp.AddDays(1));
        Check(File.GetLastAccessTimeUtc(file) == stamp.AddDays(1) && File.GetLastWriteTimeUtc(file) == stamp, "access time set alone");
        // A copy takes the permission bits and the write time of its source with it.
        string copied = Path.Combine(root, "copied.txt");
        File.Copy(file, copied);
        Check(File.GetUnixFileMode(copied) == File.GetUnixFileMode(file) && File.GetLastWriteTimeUtc(copied) == stamp,
            "copy has mode " + File.GetUnixFileMode(copied) + " and write time " + File.GetLastWriteTimeUtc(copied).ToString("O"));
        File.Delete(copied);
        File.SetAttributes(file, FileAttributes.ReadOnly);
        Check((File.GetAttributes(file) & FileAttributes.ReadOnly) != 0, "read-only attribute");
        File.SetAttributes(file, FileAttributes.Normal);

        // Symbolic links: created, read back, followed, resolved.
        string link = Path.Combine(root, "link.txt");
        File.CreateSymbolicLink(link, Path.Combine("inner", "data.txt"));
        Check(new FileInfo(link).LinkTarget == Path.Combine("inner", "data.txt"), "link target '" + new FileInfo(link).LinkTarget + "'");
        Check(File.ReadAllText(link) == "content", "reading through a link");
        Check(File.ResolveLinkTarget(link, returnFinalTarget: true)?.FullName == file, "resolved link target");
        Check((File.GetAttributes(link) & FileAttributes.ReparsePoint) != 0, "link attribute");
        string dangling = Path.Combine(root, "dangling");
        File.CreateSymbolicLink(dangling, Path.Combine(root, "nowhere"));
        // The BCL counts a link whose target is missing as an existing file; opening it is what fails.
        Check(new FileInfo(dangling).LinkTarget == Path.Combine(root, "nowhere") && Throws<FileNotFoundException>(() => File.ReadAllText(dangling)), "dangling link");
        Check(new FileInfo(file).LinkTarget == null, "a regular file has no link target");
        File.Delete(link);
        Check(File.Exists(file), "deleting a link keeps its target");

        // The working directory.
        string before = Directory.GetCurrentDirectory();
        Directory.SetCurrentDirectory(Path.Combine(root, "inner"));
        Check(File.ReadAllText("data.txt") == "content", "relative path after changing the working directory");
        Check(Path.GetFileName(Directory.GetCurrentDirectory()) == "inner", "working directory " + Directory.GetCurrentDirectory());
        Directory.SetCurrentDirectory(before);

        // Locks: FileShare.None excludes a second open, and a locked range excludes another handle's lock.
        using (var exclusive = new FileStream(file, FileMode.Open, FileAccess.ReadWrite, FileShare.None))
        {
            Check(Throws<IOException>(() => new FileStream(file, FileMode.Open, FileAccess.Read, FileShare.Read).Dispose()), "second open of an exclusively opened file");
        }
        using (var first = new FileStream(file, FileMode.Open, FileAccess.ReadWrite, FileShare.ReadWrite))
        using (var second = new FileStream(file, FileMode.Open, FileAccess.ReadWrite, FileShare.ReadWrite))
        {
            first.Lock(0, 4);
            Check(Throws<IOException>(() => second.Lock(2, 4)), "overlapping range lock");
            second.Lock(4, 3);
            first.Unlock(0, 4);
            second.Lock(0, 4);
        }
        Console.WriteLine("links, modes, times, working directory and locks pass");
#else
        Check(Throws<IOException>(() => File.CreateSymbolicLink(Path.Combine(root, "link"), file)) || Throws<PlatformNotSupportedException>(() => File.CreateSymbolicLink(Path.Combine(root, "link"), file)) || Throws<UnauthorizedAccessException>(() => File.CreateSymbolicLink(Path.Combine(root, "link"), file)), "a link was created without the operation");
        Console.WriteLine("optional file operations absent, as expected");
#endif
        Directory.Delete(root, recursive: true);
    }

    // ---- child processes ---------------------------------------------------------------------------------
    private static void Processes()
    {
#if EXPECT_PROCESSES
        // All three streams redirected, an exit code, and a line that travels in and back out.
        var info = new ProcessStartInfo("/bin/sh") { RedirectStandardInput = true, RedirectStandardOutput = true, RedirectStandardError = true };
        info.ArgumentList.Add("-c"); info.ArgumentList.Add("read line; echo \"got:$line\"; echo problem >&2; exit 7");
        using (var child = Process.Start(info)!)
        {
            Check(child.Id > 0, "child id");
            child.StandardInput.WriteLine("hello child");
            child.StandardInput.Close();
            string output = child.StandardOutput.ReadToEnd(), error = child.StandardError.ReadToEnd();
            Check(child.WaitForExit(20000), "child did not exit");
            Check(child.ExitCode == 7, "exit code " + child.ExitCode);
            Check(output == "got:hello child\n" && error == "problem\n", "child streams '" + output + "' '" + error + "'");
        }

        // The environment given replaces the parent's; the working directory applies; arguments arrive intact.
        var second = new ProcessStartInfo("/bin/sh") { RedirectStandardOutput = true, WorkingDirectory = "/tmp" };
        second.ArgumentList.Add("-c"); second.ArgumentList.Add("echo \"$PROBE_CHILD|$SYSTEM_PROBE_VALUE|$1|$2|$(pwd)\""); second.ArgumentList.Add("sh");
        second.ArgumentList.Add("two words"); second.ArgumentList.Add("");
        second.Environment.Clear(); second.Environment["PROBE_CHILD"] = "set by the parent";
        using (var child = Process.Start(second)!)
        {
            string line = child.StandardOutput.ReadToEnd();
            child.WaitForExit();
            Check(line == "set by the parent||two words||/tmp\n", "child environment and arguments '" + line + "'");
        }

        // Ending a child, the Exited event, asynchronous reads, and many children at once.
        using (var sleeper = Process.Start("/bin/sleep", "30"))
        {
            var exited = new ManualResetEventSlim();
            sleeper.EnableRaisingEvents = true;
            sleeper.Exited += (_, _) => exited.Set();
            Check(!sleeper.HasExited, "sleeper exited early");
            sleeper.Kill();
            Check(sleeper.WaitForExit(20000) && sleeper.ExitCode == 137, "killed child exit code " + (sleeper.HasExited ? sleeper.ExitCode : -1));
            Check(exited.Wait(20000), "Exited event");
        }
        var many = Enumerable.Range(0, 16).Select(i =>
        {
            var start = new ProcessStartInfo("/bin/sh") { RedirectStandardOutput = true };
            start.ArgumentList.Add("-c"); start.ArgumentList.Add("echo child " + i + "; exit " + i);
            return Process.Start(start)!;
        }).ToArray();
        int right = 0;
        for (int i = 0; i < many.Length; i++)
        {
            string text = many[i].StandardOutput.ReadToEndAsync().GetAwaiter().GetResult();
            many[i].WaitForExit();
            if (many[i].ExitCode == i && text == "child " + i + "\n") right++;
            many[i].Dispose();
        }
        Check(right == 16, "concurrent children " + right);
        Check(Throws<Win32Exception>(() => Process.Start("/no/such/program")), "starting a missing program");
        Console.WriteLine("processes children=" + (many.Length + 3));
#else
        Check(Throws<Win32Exception>(() => Process.Start("/bin/sh")) || Throws<PlatformNotSupportedException>(() => Process.Start("/bin/sh")), "a process started without the group");
        Console.WriteLine("processes absent, as expected");
#endif
    }

    // ---- requests from outside the process ---------------------------------------------------------------
    private static void Notifications()
    {
#if EXPECT_NOTIFICATIONS && EXPECT_PROCESSES
        // A real signal, sent by a child process, arrives as a managed registration's callback.
        var terminated = new ManualResetEventSlim();
        using (PosixSignalRegistration.Create(PosixSignal.SIGTERM, context => { context.Cancel = true; terminated.Set(); }))
        {
            Signal("TERM");
            Check(terminated.Wait(20000), "SIGTERM registration was not called");
        }
        var interrupted = new ManualResetEventSlim();
        ConsoleCancelEventHandler onCancel = (_, e) => { e.Cancel = true; interrupted.Set(); };
        Console.CancelKeyPress += onCancel;
        Signal("INT");
        Check(interrupted.Wait(20000), "Console.CancelKeyPress was not raised");
        Console.CancelKeyPress -= onCancel;
        int resized = 0;
        using (PosixSignalRegistration.Create(PosixSignal.SIGWINCH, _ => Interlocked.Increment(ref resized)))
        {
            Signal("WINCH");
            SpinWait.SpinUntil(() => Volatile.Read(ref resized) > 0, 20000);
            Check(resized > 0, "SIGWINCH registration was not called");
        }
        Console.WriteLine("notifications delivered");
#else
        // A registration is accepted or refused, but nothing can ever reach it.
        try { using var registration = PosixSignalRegistration.Create(PosixSignal.SIGTERM, _ => { }); Console.WriteLine("notifications: registration accepted, no source"); }
        catch (Exception e) when (e is IOException or PlatformNotSupportedException) { Console.WriteLine("notifications absent, as expected"); }
#endif
    }
#if EXPECT_NOTIFICATIONS && EXPECT_PROCESSES
    private static void Signal(string name)
    {
        var info = new ProcessStartInfo("/bin/sh");
        info.ArgumentList.Add("-c"); info.ArgumentList.Add("kill -" + name + " " + Environment.ProcessId);
        using var sender = Process.Start(info)!;
        sender.WaitForExit();
        Check(sender.ExitCode == 0, "kill -" + name + " exit code " + sender.ExitCode);
    }
#endif

    // ---- native libraries ------------------------------------------------------------------------------------
    private static void Modules()
    {
#if EXPECT_MODULES
        IntPtr library = NativeLibrary.Load("libm.so.6");
        var cosine = (delegate* unmanaged<double, double>)NativeLibrary.GetExport(library, "cos");
        Check(Math.Abs(cosine(0) - 1) < 1e-12, "cos(0) through a loaded library");
        Check(!NativeLibrary.TryGetExport(library, "no_such_symbol_here", out _), "a missing export was found");
        NativeLibrary.Free(library);
        Check(!NativeLibrary.TryLoad("libno-such-library.so", out _), "a missing library loaded");
        Console.WriteLine("modules loaded");
#else
        Check(!NativeLibrary.TryLoad("libm.so.6", out _), "a library loaded without module loading");
        Console.WriteLine("modules absent, as expected");
#endif
    }

    static int Main()
    {
        Console.WriteLine("SYSTEM PROBE start");
        SystemFacts();
        Links();
        Processes();
        Notifications();
        Modules();
        Console.WriteLine(s_failures == 0 ? "SYSTEM PROBE PASS" : "SYSTEM PROBE FAIL count=" + s_failures);
        return s_failures == 0 ? 0 : 1;
    }
}

// System.Console on an interactive terminal. The driver (scripts/terminal-probe.sh) owns the other side of a
// pseudo-terminal: it sizes the window, waits for each "READY" line and types what the step asks for.
static class Program
{
    private static int s_failures;
    private static void Check(bool ok, string what) { if (!ok) { s_failures++; Console.Error.WriteLine("FAIL " + what); } }
    private static void Ready(string step) { Console.Out.WriteLine("READY " + step); Console.Out.Flush(); }

    private static int Main()
    {
        Check(!Console.IsInputRedirected && !Console.IsOutputRedirected && !Console.IsErrorRedirected, "a stream is reported as redirected");
        Console.WriteLine("TERMINAL PROBE start size=" + Console.WindowWidth + "x" + Console.WindowHeight);
        Check(Console.WindowWidth == 132 && Console.WindowHeight == 43, "window " + Console.WindowWidth + "x" + Console.WindowHeight);

        // Single keys, without echo and without waiting for Enter.
        Check(!Console.KeyAvailable, "a key is available before anything was typed");
        Ready("keys");
        ConsoleKeyInfo first = Console.ReadKey(intercept: true);
        Check(first.KeyChar == 'a' && first.Key == ConsoleKey.A && first.Modifiers == 0, "first key " + first.Key + " '" + first.KeyChar + "' " + first.Modifiers);
        ConsoleKeyInfo second = Console.ReadKey(intercept: true);
        Check(second.KeyChar == 'Z' && second.Key == ConsoleKey.Z && second.Modifiers == ConsoleModifiers.Shift, "second key " + second.Key + " '" + second.KeyChar + "' " + second.Modifiers);

        // A key that is there is seen before it is read.
        Ready("available");
        for (int waited = 0; !Console.KeyAvailable && waited < 5000; waited += 10) Thread.Sleep(10);
        Check(Console.KeyAvailable, "KeyAvailable stayed false after a key was typed");
        Check(Console.ReadKey(intercept: true).KeyChar == 'k', "the key that was available");

        // Ctrl+C read as a key.
        Console.TreatControlCAsInput = true;
        Check(Console.TreatControlCAsInput, "TreatControlCAsInput does not read back");
        Ready("control-c-as-input");
        ConsoleKeyInfo control = Console.ReadKey(intercept: true);
        Check(control.Key == ConsoleKey.C && control.Modifiers == ConsoleModifiers.Control, "Ctrl+C as input gave " + control.Key + " " + control.Modifiers);
        Console.TreatControlCAsInput = false;

        // Ctrl+C as an interrupt that the program cancels.
        using var interrupted = new ManualResetEventSlim();
        ConsoleCancelEventHandler handler = (_, e) => { e.Cancel = true; interrupted.Set(); };
        Console.CancelKeyPress += handler;
        Ready("control-c-as-interrupt");
        Check(interrupted.Wait(10000), "CancelKeyPress was not raised");
        Console.CancelKeyPress -= handler;

        // A line, edited by the terminal itself.
        Ready("line");
        string? line = Console.ReadLine();
        Check(line == "hello", "line '" + line + "'");

        // A resized window is noticed.
        Ready("resize");
        for (int waited = 0; Console.WindowWidth != 100 && waited < 5000; waited += 10) Thread.Sleep(10);
        Check(Console.WindowWidth == 100 && Console.WindowHeight == 30, "window after the resize " + Console.WindowWidth + "x" + Console.WindowHeight);

        // The probe ends while the terminal is in raw mode on purpose: the driver checks that it got its line mode back.
        Ready("last-key");
        Check(Console.ReadKey(intercept: true).KeyChar == 'q', "last key");
        Console.WriteLine(s_failures == 0 ? "TERMINAL PROBE PASS" : "TERMINAL PROBE FAIL failures=" + s_failures);
        return s_failures == 0 ? 0 : 1;
    }
}

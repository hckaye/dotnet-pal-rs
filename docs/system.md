# System facts, notifications, child processes and the terminal

Four capability groups follow `faults` in `dotnet_pal_api`. The boundary's System.Native
builds on them the environment as an enumeration and the facts behind `Environment` and
`RuntimeInformation`, POSIX signal registrations and `Console.CancelKeyPress`,
`System.Diagnostics.Process`, and the interactive console. A port implements each group as
one Rust trait, or leaves it absent.

| Group | Capability bit | Trait | Callbacks |
| --- | --- | --- | --- |
| `system` | `DOTNET_PAL_CAP_SYSTEM` | `port::SystemInfo` | `environment_entry`, `text`, `process_times`, `uptime_ns`, `user_ids` |
| `notifications` | `DOTNET_PAL_CAP_NOTIFICATIONS` | `port::Notifications` | `install`, `enable`, `disable`, `default_action` |
| `processes` | `DOTNET_PAL_CAP_PROCESSES` | `port::Processes` | `spawn`, `wait`, `terminate`, `release`, `pipe_read`, `pipe_write`, `pipe_close` |
| `terminal` | `DOTNET_PAL_CAP_TERMINAL` | `port::Terminal` | `window_size`, `set_input_mode`, `input_ready`, `control_character` |

## System facts

`environment_entry(index)` enumerates the environment snapshot as `NAME=value` texts and
reports `NOT_FOUND` past the last one; the `runtime` group's lookup by name stays what
the runtime itself uses. `text(what)` answers one question per selector: the path of the
running executable, the operating system's name, release and version, the name and the
home directory of the user the process runs as. `process_times` is the CPU time the
process has consumed, split into user and kernel time, `uptime_ns` the time since the
machine started, `user_ids` the numeric user and primary group.

Every question is optional per call. A target that cannot answer one reports
`UNSUPPORTED` and keeps the capability; the bare-metal example names its machine and its
uptime and has no user, no environment and no process accounting.

System.Native builds `GetEnviron`, `GetProcessPath`, `GetUnixRelease`, `GetUnixVersion`,
`GetEUid`, `GetEGid`, `GetCpuUtilization` and `GetBootTimeTicks` on it. The passwd lookups
know exactly one user, the one the process runs as; any other user or name is "no such
entry". Groups other than the primary one, sessions and priorities are not carried.

## Notifications

A notification is a request that reaches the process from outside it: the interrupt key,
a request to quit or to terminate, a lost controlling terminal, a resumed process, a
resized terminal window, the job-control stops. The port owns the mechanism. On a POSIX
target that is nine signals; elsewhere it is a console control handler or a button.

The consumer installs one handler, once, and enables the kinds it wants. The port calls
`dotnet_pal_rs::notifications::deliver(kind)` for an enabled kind from a thread of its
own, never from a signal or interrupt context, so the handler may use the whole boundary
and may enter managed code. A kind that is not enabled keeps the action it had before.
`default_action(kind)` performs that action for a kind the consumer saw and chose not to
handle; for `TERMINATE` that normally ends the process the way the target would have.
`deliver` answers whether the consumer took the report. When it did not, because the kind
was disabled in the meantime, the earlier action is the port's to take.

```rust
// A port whose mechanism is an interrupt: the handler only records the event...
fn interrupt_key_irq() { PENDING.store(true, Ordering::Release); wake(DISPATCHER); }
// ...and a thread of the port reports it.
fn dispatcher() { loop { wait(); if PENDING.swap(false, Ordering::AcqRel) { dotnet_pal_rs::notifications::deliver(notifications::INTERRUPT); } } }
```

System.Native keeps the signal numbers as the vocabulary of the managed side
(`PosixSignalRegistration`, `Console.CancelKeyPress`): `EnablePosixSignalHandling(SIGTERM)`
enables `TERMINATE`, a delivered kind calls the managed handler with the signal number,
and a registration that does not cancel ends in `default_action`. `SIGCHLD` needs no
notification; the process unit reports child ends itself.

## Child processes

`spawn` starts a program with an argument vector, an environment (or the parent's, when
none is given), a working directory and a choice of which of the child's three standard
streams are pipes to the parent. The program is a path and is never searched for; a
relative one is resolved in the child's working directory. Every environment entry is
`NAME=value` with a name. The child starts with no signal blocked and every signal at
its default action, and it inherits none of the boundary's handles except its standard
streams. The result is a process handle, an identifier and one pipe handle per requested
stream. `wait` blocks until the child has ended or the timeout
expires (`TIMEOUT`) and reports the exit code, or 128 plus the signal number for a child
that a signal ended; it may be asked again. `terminate` asks the child to end or ends
it, and reports `NOT_FOUND` for a child it knows to have ended. `release` gives the
handle back and does not end the child; the exit of a child released while it runs is
reported to nobody. Pipes block. A pipe read reports zero bytes once the other end is
closed; a pipe write reports `BROKEN_PIPE` then, and never raises a signal.

The BCL's `Process` class starts a child with one call that also makes the pipes, learns
that a child has ended from `SIGCHLD`, and collects the exit code by process id.
`native/system_native_proc.c` gives each child a watcher thread instead: it blocks in
`wait`, records the exit code and then calls the callback `Process` registered, exactly
where the reference implementation's signal thread would have. The pipes become
descriptors of the same table that holds files and sockets, so `Read`, `Write`, `Dup`
and `Close` work on them. Only children started through the boundary can be waited for
or signalled; starting a child as another user is not carried.

## The terminal

`window_size(stream)` is the size of the terminal a standard stream is connected to.
`set_input_mode` switches input between line mode (the terminal edits and echoes a line
and hands it over at Enter) and raw mode (every byte as typed, without echo; a read
returns once `min_bytes` have arrived or `timeout_ds` tenths of a second have passed),
and says whether the interrupt key is delivered as a byte instead of as a notification.
`input_ready` says whether a read would return now. `control_character` is the byte the
terminal uses for erase, end of line and end of file, or `NOT_FOUND`. A stream that is
no terminal is `NOT_FOUND`, and a terminal that nobody gave a size is `UNSUPPORTED` for
`window_size`. A process in the background of its terminal is subject to the target's
job control when it changes the mode: the POSIX providers let `SIGTTOU` stop it until it
is in the foreground, as the reference implementation does for a read, because the
terminal belongs to the foreground job.

The POSIX providers derive every mode from the settings the terminal had when they first
looked. Raw mode clears flow control, the translation of carriage return and newline,
echo, line editing and extended input processing, the same flags the reference
implementation clears. Line mode is those first settings again, with the interrupt key
on or off as asked.

`Console.ReadKey`, `Console.KeyAvailable`, `Console.WindowWidth` and
`Console.TreatControlCAsInput` rest on these through `InitializeConsoleBeforeRead`,
`StdinReady`, `GetWindowSize` and `SetSignalForBreak`. `TreatControlCAsInput` starts as
false whatever the terminal was set to, because the group has no call that reads the
state of the interrupt key. System.Native puts the terminal back into line mode with the
interrupt key on when the process exits, so a program that ends while it reads keys does
not leave its terminal without echo.

## Module loading

`NativeLibrary.Load`, `GetExport` and `Free` go through the `runtime` group's module
callbacks (`SystemNative_LoadLibrary` and its companions in `native/system_native_sys.c`).
A port without `Modules` loads nothing.

## Providers

| Provider | `SystemInfo` | `Notifications` | `Processes` | `Terminal` |
| --- | --- | --- | --- | --- |
| Linux backend (`linux` feature) | `src/linux_system.rs` | `src/linux_notifications.rs`: signals through a self-pipe to a dispatcher thread | `src/linux_processes.rs`: `posix_spawn`, waits through a pidfd, or by polling where the kernel has none | `src/linux_terminal.rs`: termios |
| Desktop `std` port | `std::env`, `libc` for the rest | the same design with `std::thread` | `std::process::Command` | termios through `libc` |
| C host tables | `host-system` | `host-notifications` | `host-processes` | `host-terminal` |
| Bare-metal example | machine name, uptime, image path | absent | absent | absent |

The Linux processes provider sets the child's working directory with
`posix_spawn_file_actions_addchdir_np`, which needs glibc 2.29 or musl 1.1.24 at link
time.

`scripts/system.sh`, `notifications.sh`, `processes.sh` and `terminal.sh` run the
conformance tests of the Linux providers and of independent C providers behind host
tables: real signals sent to the process, real children with pipes, and a
pseudo-terminal the test creates for itself. `samples/SystemProbe` is the managed
counterpart; `scripts/system-probe.sh` runs it on Linux with the boundary's System.Native
and the isolation gate, and `examples/baremetal-aarch64/build-app.sh SystemProbe` runs it
with no OS, where links, modes, times, locks and the working directory work on the
in-memory file system ([io](io.md)) and child processes, notifications and module loading
are absent.

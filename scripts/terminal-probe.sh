#!/usr/bin/env bash
# Publish the terminal probe (System.Console on an interactive terminal: window size,
# key reads, KeyAvailable, Ctrl+C as input and as CancelKeyPress, line reads, a resized
# window) against the source-built runtime and the boundary's System.Native, and run
# it on Linux on a pseudo-terminal whose other side this script plays. The terminal
# must be back in line mode with echo and the interrupt key once the probe has ended.
set -euo pipefail
cd "$(dirname "$0")/.."
root="$PWD"
case "$(uname -m)" in x86_64) rid=linux-x64;; aarch64) rid=linux-arm64;; *) exit 2;; esac
[[ "$(uname -s)" == Linux ]]
overlay="$root/artifacts/source-sdk"
[[ -f "$overlay/libRuntime.WorkstationGC.a" ]] || { echo 'run scripts/source-runtime.sh first: the terminal probe links the source-built runtime' >&2; exit 1; }
mkdir -p artifacts/terminal-probe
bash scripts/system-native.sh
# The framework native directory with only System.Native replaced.
native_dir=$(dirname "$(readlink -f "$overlay/libSystem.Globalization.Native.a")")
framework="$root/artifacts/terminal-probe/native"
rm -rf "$framework"; mkdir -p "$framework"
cp -as "$native_dir/." "$framework/"
rm "$framework/libSystem.Native.a"
cp artifacts/system-native/libSystem.Native.a "$framework/libSystem.Native.a"
cargo rustc --lib --crate-type staticlib --release --features linux
rm -rf samples/TerminalProbe/obj samples/TerminalProbe/bin
dotnet publish samples/TerminalProbe/TerminalProbe.csproj -c Release -r "$rid" \
  "-p:IlcSdkPath=$overlay/" "-p:IlcFrameworkNativePath=$framework/" \
  "-p:PalLinkMap=$root/artifacts/terminal-probe/link.map" -o artifacts/terminal-probe
python3 - <<'PY'
# The other side of the pseudo-terminal: size the window, start the probe on the slave side as the controlling
# terminal of a new session, and type what each READY line asks for.
import fcntl, os, pty, re, select, signal, struct, sys, termios, time
master, slave = pty.openpty()
fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack('HHHH', 43, 132, 0, 0))
before = termios.tcgetattr(slave)
pid = os.fork()
if pid == 0:
    os.setsid()
    fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
    for fd in (0, 1, 2): os.dup2(slave, fd)
    os.close(master); os.close(slave)
    os.environ['TERM'] = 'xterm'
    os.execv('artifacts/terminal-probe/TerminalProbe', ['TerminalProbe'])
log = open('artifacts/terminal-probe/run.log', 'wb')
seen = b''
def until(marker, seconds=60):
    global seen
    deadline = time.time() + seconds
    while marker not in seen:
        if time.time() > deadline: raise SystemExit('timed out waiting for ' + marker.decode() + '; output so far: ' + seen.decode(errors='replace'))
        ready, _, _ = select.select([master], [], [], 0.2)
        if ready:
            try: data = os.read(master, 4096)
            except OSError: data = b''
            if not data: raise SystemExit('the probe ended before ' + marker.decode() + '; output: ' + seen.decode(errors='replace'))
            seen += data; log.write(data); log.flush()
            # A terminal answers a request for the cursor position, which the BCL sends when it echoes a line.
            for _ in range(data.count(b'\x1b[6n')): os.write(master, b'\x1b[1;1R')
    seen = seen.split(marker, 1)[1]
def type_(text, pause=0.15):
    time.sleep(pause)
    os.write(master, text)
until(b'READY keys'); type_(b'a'); type_(b'Z')
until(b'READY available'); type_(b'k')
until(b'READY control-c-as-input'); type_(b'\x03')
until(b'READY control-c-as-interrupt'); type_(b'\x03', 0.5)
until(b'READY line'); type_(b'hellp\x7fo\r')   # a typing error and an erase: the terminal edits the line
until(b'READY resize'); time.sleep(0.2)
fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack('HHHH', 30, 100, 0, 0))   # the kernel sends SIGWINCH to the foreground group
until(b'READY last-key')
print('lflag at the last key: %#x' % termios.tcgetattr(slave)[3])
# The READY line comes before the read that switches the terminal, so the switch is waited for.
deadline = time.time() + 10
raw = termios.tcgetattr(slave)
while raw[3] & (termios.ICANON | termios.ECHO) and time.time() < deadline:
    time.sleep(0.02); raw = termios.tcgetattr(slave)
if raw[3] & (termios.ICANON | termios.ECHO): raise SystemExit('the terminal is not in raw mode while the probe waits for a key')
type_(b'q')
until(b'TERMINAL PROBE PASS')
_, status = os.waitpid(pid, 0)
if not os.WIFEXITED(status) or os.WEXITSTATUS(status) != 0: raise SystemExit('the probe ended with status ' + str(status))
after = termios.tcgetattr(slave)
wanted = termios.ICANON | termios.ECHO | termios.ISIG
if after[3] & wanted != wanted: raise SystemExit('the probe left its terminal without line mode, echo or the interrupt key: lflag=%#x' % after[3])
print('terminal restored: lflag %#x before, %#x while reading a key, %#x after' % (before[3], raw[3], after[3]))
PY
grep -aq 'TERMINAL PROBE PASS' artifacts/terminal-probe/run.log
python3 - <<'PY'
import re
from pathlib import Path
text = Path('artifacts/terminal-probe/link.map').read_text(errors='replace')
# GNU ld map: an input section line names the symbol, the next line names the object that supplied it.
providers = {name: origin for name, origin in re.findall(r'\.text\.(SystemNative_\w+)\s*\n\s*0x[0-9a-f]+\s+0x[0-9a-f]+\s+(\S+)', text)}
if not providers: raise SystemExit('no SystemNative sections in the link map: the map format changed')
foreign = sorted(name for name, origin in providers.items() if not re.search(r'libSystem\.Native\.a\(system_native_(pal|io|net|sys|proc)\.o\)', origin))
print(f"SystemNative symbols linked: {len(providers)}; from the boundary objects: {len(providers) - len(foreign)}")
if foreign: raise SystemExit('SystemNative symbols not provided by native/system_native_*.c: ' + ', '.join(foreign))
for needed in ('SystemNative_InitializeConsoleBeforeRead', 'SystemNative_GetWindowSize', 'SystemNative_StdinReady', 'SystemNative_SetSignalForBreak',
               'SystemNative_InitializeTerminalAndSignalHandling'):
    if needed not in providers: raise SystemExit(needed + ' is not in the image: the probe no longer reaches it')
PY
python3 scripts/audit_dependencies.py --runtime "$overlay/libRuntime.WorkstationGC.a" --runtime "$overlay/libaotminipal.a" \
  --runtime artifacts/system-native/libSystem.Native.a --pal target/release/libdotnet_pal_rs.a \
  --binary artifacts/terminal-probe/TerminalProbe --output artifacts/terminal-probe/dependency-inventory.log --require-isolated
echo "TERMINAL PROBE PASS rid=$rid: System.Console on a pseudo-terminal reaches the OS only through the boundary"

#!/usr/bin/env python3
"""Inventory actual archive references and executable imports, without hiding bypasses.

An archive's unresolved set is not the executable's import set: symbols may resolve
from another archive, and unused archive members may not be linked. Report both.
--require-isolated is an intentionally strict release gate, not enabled implicitly.
"""
import argparse
from collections import defaultdict
import hashlib
import json
from pathlib import Path
import re
import subprocess

MEMORY = set('mmap mmap64 munmap mprotect madvise msync mlock munlock mremap getpagesize sbrk brk posix_memalign aligned_alloc memalign malloc calloc realloc free'.split())
CLOCK = set('clock_gettime clock_getres clock_nanosleep nanosleep gettimeofday sleep usleep sched_yield'.split())
MODULE = set('dlopen dlclose dlsym dlerror dladdr dl_iterate_phdr dlinfo'.split())
PROCESS = set('sysconf sysinfo getrlimit setrlimit prlimit getrusage getpid getppid gettid getenv setenv unsetenv putenv environ fork execve waitpid exit _exit abort sched_getaffinity sched_setaffinity sched_getcpu getauxval getrandom random arc4random'.split())
FILES = set('open open64 openat close read write pread pwrite pread64 pwrite64 readv writev lseek lseek64 fstat fstat64 stat stat64 lstat lstat64 statfs fstatfs fcntl ioctl dup dup2 dup3 pipe pipe2 access faccessat unlink unlinkat rename renameat mkdir mkdirat rmdir opendir readdir closedir readlink readlinkat chdir getcwd truncate ftruncate fsync fdatasync chmod fchmod chown fchown utimensat fopen fclose fread fwrite fflush fseek ftell fputs fputc fprintf printf snprintf sscanf getline fileno poll ppoll select pselect epoll_create epoll_create1 epoll_ctl epoll_wait eventfd'.split())
NETWORK = set('socket socketpair connect bind listen accept accept4 shutdown send sendto recv recvfrom sendmsg recvmsg getsockopt setsockopt getaddrinfo freeaddrinfo getnameinfo gai_strerror gethostname inet_pton inet_ntop'.split())


def classify(name):
    plain = name.split('@')[0]
    if plain == 'dotnet_pal_get_api' or plain.startswith('dotnet_pal_host_'): return 'boundary'
    if plain.startswith(('sig', 'pthread_sig')) or plain in ('getcontext', 'setcontext', 'swapcontext', 'makecontext', 'kill', 'raise'):
        return 'signals-and-context'
    if plain.startswith(('pthread_', 'sem_')): return 'threads-and-synchronization'
    if plain in MEMORY: return 'memory-and-native-allocator'
    if plain in CLOCK: return 'clock-and-scheduling'
    if plain in MODULE: return 'module-and-loader'
    if plain in PROCESS: return 'process-environment-and-topology'
    if plain in FILES or plain.startswith('__xstat'): return 'files-and-io'
    if plain in NETWORK: return 'network'
    if plain == 'syscall': return 'raw-syscall-needs-callsite-review'
    if plain.startswith(('__aeabi_', '__div', '__udiv', '__mul', '__fix', '__float', '__stack_chk', '__cxa_', '__gxx_', '_Unwind_')):
        return 'compiler-crt-or-unwind'
    return 'other-needs-review'


def parse_nm(text):
    result = []
    # GNU and LLVM -A -P both emit owner: name type [value [size]].
    pattern = re.compile(r'^(.*?):\s+(\S+)\s+([A-Za-z?])(?:\s.*)?$')
    for line in text.splitlines():
        match = pattern.match(line)
        if match:
            owner, symbol, kind = match.groups()
            result.append((owner, symbol, kind))
        elif line.strip() and not line.rstrip().endswith(':'):
            raise ValueError('unrecognized nm line: ' + line[:200])
    return result


def inventory(path, nm, dynamic=False):
    command = [nm, '-A', '-P', '-g']
    if dynamic: command.append('-D')
    command.append(str(path))
    process = subprocess.run(command, check=True, capture_output=True, text=True)
    rows = parse_nm(process.stdout)
    definitions = {symbol for _, symbol, kind in rows if kind not in ('U', 'w', 'v', '?')}
    references = defaultdict(set)
    weak = defaultdict(set)
    for owner, symbol, kind in rows:
        if kind == 'U' and symbol not in definitions: references[symbol].add(owner)
        if kind in ('w', 'v') and symbol not in definitions: weak[symbol].add(owner)
    with path.open('rb') as stream: digest = hashlib.file_digest(stream, 'sha256').hexdigest()
    items = [{'symbol': symbol, 'category': classify(symbol), 'owners': sorted(owners)}
             for symbol, owners in sorted(references.items())]
    return {'path': str(path), 'sha256': digest, 'dynamic_symbols_only': dynamic,
            'unresolved_strong': items, 'unresolved_weak': {k: sorted(v) for k, v in sorted(weak.items())},
            'tool_warnings': process.stderr}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--runtime', type=Path, required=True)
    parser.add_argument('--pal', type=Path, required=True)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--nm', default='nm')
    parser.add_argument('--require-isolated', action='store_true')
    args = parser.parse_args()
    report = {'schema': 1,
              'scope': 'link references; not a syscall trace, call-graph proof, or proof of full OS independence',
              'runtime': inventory(args.runtime.resolve(), args.nm),
              'rust_pal': inventory(args.pal.resolve(), args.nm),
              'executable': inventory(args.binary.resolve(), args.nm, dynamic=True)}
    boundary_calls = [i for i in report['runtime']['unresolved_strong'] if i['category'] == 'boundary']
    if not boundary_calls: raise SystemExit('rebuilt runtime does not reference the PAL entry point')
    os_categories = {'signals-and-context', 'threads-and-synchronization', 'memory-and-native-allocator',
        'clock-and-scheduling', 'module-and-loader', 'process-environment-and-topology',
        'files-and-io', 'network', 'raw-syscall-needs-callsite-review'}
    bypasses = [i for i in report['runtime']['unresolved_strong'] if i['category'] in os_categories]
    report['runtime_os_references_outside_boundary'] = bypasses
    report['isolated_runtime'] = not bypasses and all(i['category'] != 'other-needs-review' for i in report['runtime']['unresolved_strong'])
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + '\n')
    print(f"DEPENDENCY INVENTORY runtime_external={len(report['runtime']['unresolved_strong'])} remaining_os_references={len(bypasses)} output={args.output}")
    print('REMAINING OS REFERENCES: ' + ', '.join(i['symbol'] for i in bypasses))
    if args.require_isolated and not report['isolated_runtime']:
        raise SystemExit('runtime is not OS-isolated; do not label the whole runtime port complete')


if __name__ == '__main__': main()

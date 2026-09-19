#!/usr/bin/env python3
"""Inventory actual archive references and executable imports, without hiding bypasses.

An archive's unresolved set is not the executable's import set: symbols may resolve
from another archive, and unused archive members may not be linked. Report both.
--require-isolated is an intentionally strict release gate, not enabled implicitly:
it passes only when every external reference of the runtime archive set is the
boundary entry point or a reviewed non-OS contract listed in
integration/dotnet10/reviewed_references.json (compiler output, C++ ABI, compiler
builtins, the freestanding C runtime, libm, event tracing, assembler artifacts).
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
PROCESS = set('sysconf sysinfo getrlimit getrlimit64 setrlimit prlimit getrusage getpid getppid gettid getenv setenv unsetenv putenv environ fork execv execve waitpid _exit sched_getaffinity sched_setaffinity sched_getcpu getauxval getrandom random arc4random arc4random_buf prctl __sched_cpualloc __sched_cpucount __sched_cpufree'.split())
FILES = set('open open64 openat close read write pread pwrite pread64 pwrite64 readv writev lseek lseek64 fstat fstat64 stat stat64 lstat lstat64 statfs statfs64 fstatfs fcntl ioctl dup dup2 dup3 pipe pipe2 access faccessat unlink unlinkat rename renameat mkdir mkdirat rmdir opendir readdir readdir64 closedir readlink readlinkat chdir getcwd truncate ftruncate fsync fdatasync chmod fchmod chown fchown utimensat fopen fopen64 fclose fread fwrite fflush fseek ftell fputs fputc fprintf printf getline __getdelim asprintf vfprintf vprintf fileno stderr stdout stdin poll ppoll select pselect epoll_create epoll_create1 epoll_ctl epoll_wait eventfd'.split())
NETWORK = set('socket socketpair connect bind listen accept accept4 shutdown send sendto recv recvfrom sendmsg recvmsg getsockopt setsockopt getaddrinfo freeaddrinfo getnameinfo gai_strerror gethostname inet_pton inet_ntop'.split())


REVIEWED_MANIFEST = Path(__file__).resolve().parents[1] / 'integration/dotnet10/reviewed_references.json'


def load_reviewed(path=REVIEWED_MANIFEST):
    """The reviewed non-OS contracts: category -> (exact names, compiled patterns, owner suffixes)."""
    data = json.loads(Path(path).read_text())
    result = {}
    for category, entry in data['categories'].items():
        result[category] = (set(entry.get('symbols', [])), [re.compile(p) for p in entry.get('patterns', [])], entry.get('owners'))
    return result


def reviewed_category(name, owners, reviewed):
    """The manifest category of a non-OS reference, or None when it is unreviewed."""
    plain = name.split('@')[0]
    for category, (names, patterns, owner_suffixes) in reviewed.items():
        if plain in names or any(p.search(plain) for p in patterns):
            if owner_suffixes and not all(any(o.endswith(s) or o.endswith(s + ']') for s in owner_suffixes) for o in owners):
                continue
            return category
    return None


def classify(name):
    plain = name.split('@')[0]
    if plain == 'dotnet_pal_get_api': return 'boundary'
    if plain.startswith(('dotnet_pal_host_', 'dotnet_pal_storage_')):
        return 'backend-hook-bypasses-runtime-boundary'
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
    return {'path': str(path), 'sha256': digest, 'dynamic_symbols_only': dynamic, 'defined': sorted(definitions),
            'unresolved_strong': items, 'unresolved_weak': {k: sorted(v) for k, v in sorted(weak.items())},
            'tool_warnings': process.stderr}


def merge(inventories):
    """One reference set for an archive set: a name defined by any member is internal."""
    if len(inventories) == 1: return inventories[0]
    defined = set()
    for inv in inventories: defined.update(inv.get('defined', []))
    strong = {}
    weak = {}
    for inv in inventories:
        for item in inv['unresolved_strong']:
            if item['symbol'] in defined: continue
            entry = strong.setdefault(item['symbol'], {'symbol': item['symbol'], 'category': item['category'], 'owners': []})
            entry['owners'] = sorted(set(entry['owners']) | set(item['owners']))
        for symbol, owners in inv['unresolved_weak'].items():
            if symbol in defined: continue
            weak[symbol] = sorted(set(weak.get(symbol, [])) | set(owners))
    return {'path': ' + '.join(inv['path'] for inv in inventories), 'sha256': [inv['sha256'] for inv in inventories],
            'dynamic_symbols_only': False, 'unresolved_strong': [strong[k] for k in sorted(strong)], 'unresolved_weak': dict(sorted(weak.items())),
            'tool_warnings': ''.join(inv['tool_warnings'] for inv in inventories)}


def assess_runtime(runtime, reviewed=None):
    """Conservative link-reference gate, not a call-graph or syscall proof.

    Weak imports remain dependencies. Only a strong public-entrypoint reference
    establishes integration. Backend hooks belong below the runtime boundary.
    Compiler/CRT/unwind names classify symbols, but do not approve their behavior.
    A non-OS reference counts as reviewed only when the manifest names it.
    """
    if reviewed is None: reviewed = load_reviewed()
    references = [
        {**item, 'category': classify(item['symbol']), 'binding': 'strong'}
        for item in runtime['unresolved_strong']
    ]
    references.extend(
        {'symbol': symbol, 'category': classify(symbol),
         'owners': sorted(owners), 'binding': 'weak'}
        for symbol, owners in sorted(runtime['unresolved_weak'].items())
    )
    for item in references:
        if item['category'] != 'boundary':
            item['reviewed'] = reviewed_category(item['symbol'], item['owners'], reviewed)
    has_entrypoint = any(
        item['binding'] == 'strong'
        and item['symbol'].split('@', 1)[0] == 'dotnet_pal_get_api'
        for item in references
    )
    os_categories = {
        'signals-and-context', 'threads-and-synchronization',
        'memory-and-native-allocator', 'clock-and-scheduling',
        'module-and-loader', 'process-environment-and-topology',
        'files-and-io', 'network', 'raw-syscall-needs-callsite-review',
        'backend-hook-bypasses-runtime-boundary',
    }
    bypasses = [i for i in references if i['category'] in os_categories]
    unreviewed = [i for i in references
                  if i['category'] not in os_categories and i['category'] != 'boundary' and not i.get('reviewed')]
    accepted = [i for i in references if i['category'] not in os_categories and i.get('reviewed')]
    return {
        'has_runtime_entrypoint': has_entrypoint,
        'runtime_os_references_outside_boundary': bypasses,
        'runtime_unreviewed_references_outside_boundary': unreviewed,
        'runtime_reviewed_references': accepted,
        'isolated_runtime': has_entrypoint and not bypasses and not unreviewed,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--runtime', type=Path, required=True, action='append',
                        help='runtime archive; repeat for every archive of the audited set (the GC runtime and minipal)')
    parser.add_argument('--pal', type=Path, required=True)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--nm', default='nm')
    parser.add_argument('--require-isolated', action='store_true')
    args = parser.parse_args()
    report = {'schema': 1,
              'scope': 'link references; not a syscall trace, call-graph proof, or proof of full OS independence',
              'runtime': merge([inventory(path.resolve(), args.nm) for path in args.runtime]),
              'rust_pal': inventory(args.pal.resolve(), args.nm),
              'executable': inventory(args.binary.resolve(), args.nm, dynamic=True)}
    report.update(assess_runtime(report['runtime']))
    bypasses = report['runtime_os_references_outside_boundary']
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + '\n')
    unreviewed = report['runtime_unreviewed_references_outside_boundary']
    print(f"DEPENDENCY INVENTORY runtime_external={len(report['runtime']['unresolved_strong'])} remaining_os_references={len(bypasses)} unreviewed={len(unreviewed)} output={args.output}")
    print('REMAINING OS REFERENCES: ' + ', '.join(i['symbol'] for i in bypasses))
    print('UNREVIEWED REFERENCES: ' + ', '.join(i['symbol'] for i in unreviewed))
    if not report['has_runtime_entrypoint']:
        raise SystemExit('rebuilt runtime lacks a strong reference to dotnet_pal_get_api')
    if args.require_isolated and not report['isolated_runtime']:
        raise SystemExit('runtime is not OS-isolated; do not label the whole runtime port complete')


if __name__ == '__main__': main()

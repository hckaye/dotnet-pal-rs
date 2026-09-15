#!/usr/bin/env python3
"""Source-level WASIp1 eager-storage integration for one audited LLVM revision."""
import argparse
from pathlib import Path
import re
import subprocess

REVISION = '9954350a58ede8b8eaaeb24112ca4f1e78cc527c'
WASM = 'src/coreclr/gc/wasm/gcenv.wasm.cpp'
UNIX = 'src/coreclr/gc/unix/gcenv.unix.cpp'
CMAKE = 'src/coreclr/nativeaot/Runtime/CMakeLists.txt'
GEN = 'eng/native/gen-buildsys.sh'
VM_CALLS = {
    'VirtualReserve': 'dotnet_pal_gc_linear::reserve(size, alignment, flags, node)',
    'VirtualCommit': 'dotnet_pal_gc_linear::commit(address, size, node)',
    'VirtualDecommit': 'dotnet_pal_gc_linear::decommit(address, size)',
    'VirtualRelease': 'dotnet_pal_gc_linear::release(address, size)',
    'VirtualReset': 'dotnet_pal_gc_linear::reset(address, size, unlock)',
    'VirtualReserveAndCommitLargePages': 'dotnet_pal_gc_linear::large_pages(size, node)',
}
CLOCK_CALLS = {
    'QueryPerformanceCounter': 'static_cast<int64_t>(dotnet_pal_llvm_clock::now())',
    'QueryPerformanceFrequency': 'INT64_C(1000000000)',
    'GetLowPrecisionTimeStamp': 'dotnet_pal_llvm_clock::now() / UINT64_C(1000000)',
}


def replace_definitions(text, calls):
    for name, expression in calls.items():
        regex = re.compile(r'((?:void\*|bool|int64_t|uint64_t) GCToOSInterface::' + name + r'\([^\n]*\)\n\{\n).*?^\}', re.S | re.M)
        if len(list(regex.finditer(text))) != 1:
            raise ValueError('expected exactly one audited definition: ' + name)
        text = regex.sub(lambda m: m[1] + '    return ' + expression + ';\n}', text)
    return text


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('runtime', type=Path)
    parser.add_argument('--check', action='store_true')
    args = parser.parse_args()
    root = args.runtime.resolve()
    revision = subprocess.check_output(['git', '-C', str(root), 'rev-parse', 'HEAD'], text=True).strip()
    if revision != REVISION: raise SystemExit('runtime revision mismatch; re-audit the source')
    subprocess.run(['git', '-C', str(root), 'diff', '--exit-code', 'HEAD', '--', WASM, UNIX, CMAKE, GEN], check=True)
    wasm = (root / WASM).read_text()
    unix = (root / UNIX).read_text()
    cmake = (root / CMAKE).read_text()
    gen = (root / GEN).read_text()
    if 'DOTNET_PAL_LLVM_LINEAR' in cmake: raise SystemExit('source already patched')
    wasm = replace_definitions(wasm, VM_CALLS)
    helper = re.compile(r'^static void\* VirtualReserveInner\([^\n]*\)\n\{\n.*?^\}', re.S | re.M)
    if len(list(helper.finditer(wasm))) != 1: raise SystemExit('unexpected upstream reserve helper')
    wasm = helper.sub('', wasm)
    wasm = '#include "gc_linear_adapter.h"\n' + wasm
    unix = '#include "llvm_clock_adapter.h"\n' + replace_definitions(unix, CLOCK_CALLS)
    cmake = '''# Explicit bounded linear-storage profile, not sparse VM emulation.
if(NOT CLR_CMAKE_TARGET_WASI OR NOT CLR_CMAKE_TARGET_ARCH_WASM OR NOT DOTNET_PAL_ROOT)
  message(FATAL_ERROR "This audited source overlay is restricted to the Wasm/WASI PAL profile")
endif()
set(CMAKE_CXX_STANDARD 17)
add_definitions(-DDOTNET_PAL_LLVM_LINEAR=1)
include_directories("${DOTNET_PAL_ROOT}/include" "${DOTNET_PAL_ROOT}/native")

''' + cmake
    if gen.count('/share/cmake/wasi-sdk-p2.cmake') != 1:
        raise SystemExit('unexpected WASI toolchain selection')
    gen = gen.replace('/share/cmake/wasi-sdk-p2.cmake', '/share/cmake/wasi-sdk.cmake')
    changes = {WASM: wasm, UNIX: unix, CMAKE: cmake, GEN: gen}
    if not args.check:
        for name, content in changes.items(): (root / name).write_text(content)
    print('LLVM SOURCE ADAPTER VALIDATED: six memory and three clock definitions; WASIp1 toolchain' + (' (check only)' if args.check else ' (written)'))


if __name__ == '__main__': main()

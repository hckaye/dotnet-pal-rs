#!/usr/bin/env python3
"""Prepare a NativeAOT-only SOURCE integration on the exact audited runtime commit.

This is separate from the --wrap experiment. It must be rebuilt and tested before
claiming source-port coverage. No automatic downloading, builds or git commits.
"""
import argparse
from pathlib import Path
import re
import subprocess

REVISION = "60629d14374c56f1cb51819049ad1fa529307f8d"
GC_FILE = "src/coreclr/gc/unix/gcenv.unix.cpp"
CMAKE_FILE = "src/coreclr/nativeaot/Runtime/CMakeLists.txt"
MARKER = "DOTNET_PAL_GC_VM"
CALLS = {
    "VirtualReserve": "dotnet_pal_gc::reserve(size, alignment, flags, node)",
    "VirtualCommit": "dotnet_pal_gc::commit(address, size, node)",
    "VirtualDecommit": "dotnet_pal_gc::decommit(address, size)",
    "VirtualRelease": "dotnet_pal_gc::release(address, size)",
    "VirtualReset": "dotnet_pal_gc::reset(address, size, unlock)",
    "VirtualReserveAndCommitLargePages": "dotnet_pal_gc::large_pages(size, node)",
}

HELPERS = ("VirtualReserveInner", "VirtualCommitInner")

def patch_gc(text):
    if MARKER in text:
        raise ValueError("source is already patched")
    for name, call in CALLS.items():
        # In the pinned file, top-level function closing braces start at column 0.
        # Reject missing/duplicate definitions instead of guessing a newer layout.
        pattern = rf"((?:void\*|bool) GCToOSInterface::{name}\([^\n]*\)\n\{{\n)(.*?)(^\}})"
        regex = re.compile(pattern, re.DOTALL | re.MULTILINE)
        if len(list(regex.finditer(text))) != 1:
            raise ValueError(f"expected exactly one pinned definition of {name}")
        text = regex.sub(lambda m: m[1] + f"#ifdef {MARKER}\n    return {call};\n#else\n" + m[2] + "#endif\n" + m[3], text)
    # These helpers otherwise become unused under -Werror in the source build.
    for name in HELPERS:
        regex = re.compile(rf"^static (?:void\*|bool) {name}\([^\n]*\)\n\{{\n.*?^\}}", re.DOTALL | re.MULTILINE)
        if len(list(regex.finditer(text))) != 1:
            raise ValueError(f"expected exactly one pinned helper {name}")
        text = regex.sub(lambda m: f"#ifndef {MARKER}\n" + m[0] + "\n#endif", text)
    return f'#ifdef {MARKER}\n#include "gc_vm_adapter.h"\n#endif\n\n' + text

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("runtime", type=Path, help="clean checkout of dotnet/runtime v10.0.0")
    parser.add_argument("--check", action="store_true", help="validate without writing files")
    args = parser.parse_args()
    root = args.runtime.resolve()
    head = subprocess.check_output(["git", "-C", str(root), "rev-parse", "HEAD"], text=True).strip()
    if head != REVISION:
        raise SystemExit(f"Expected {REVISION}, got {head}; re-audit before updating the pin")
    subprocess.run(["git", "-C", str(root), "diff", "--exit-code", "HEAD", "--", GC_FILE, CMAKE_FILE], check=True)
    gc = patch_gc((root / GC_FILE).read_text())
    cmake = (root / CMAKE_FILE).read_text()
    if MARKER in cmake:
        raise SystemExit("CMake is already patched")
    cmake = '''# Experimental NativeAOT-only GC VM adapter. Other runtimes remain unchanged.
if(DOTNET_PAL_ROOT)
  if(NOT CLR_CMAKE_TARGET_LINUX)
    message(FATAL_ERROR "This source integration has only been prepared for Linux")
  endif()
  add_definitions(-DDOTNET_PAL_GC_VM=1)
  include_directories("${DOTNET_PAL_ROOT}/include" "${DOTNET_PAL_ROOT}/native")
endif()

''' + cmake
    if not args.check:
        (root / GC_FILE).write_text(gc)
        (root / CMAKE_FILE).write_text(cmake)
    print("Source adapter: six GC VM definitions validated" + (" (check only)" if args.check else " and patched"))

if __name__ == "__main__":
    main()

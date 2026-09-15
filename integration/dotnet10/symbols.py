#!/usr/bin/env python3
"""Verify exact, pinned ELF C++ ABI names; fail instead of silently testing no hooks."""
import argparse
from pathlib import Path
import subprocess
import xml.etree.ElementTree as ET

SYMBOLS = (
    "_ZN15GCToOSInterface14VirtualReserveEmmjt",
    "_ZN15GCToOSInterface13VirtualCommitEPvmt",
    "_ZN15GCToOSInterface15VirtualDecommitEPvm",
    "_ZN15GCToOSInterface14VirtualReleaseEPvm",
    "_ZN15GCToOSInterface12VirtualResetEPvmb",
    "_ZN15GCToOSInterface33VirtualReserveAndCommitLargePagesEmt",
)

def defined(path):
    result = subprocess.run(["nm", "-g", "--defined-only", str(path)],
                            check=True, capture_output=True, text=True)
    return {line.split()[-1] for line in result.stdout.splitlines() if line.split()}

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--props", type=Path)
    parser.add_argument("--check-runtime", type=Path)
    args = parser.parse_args()
    if args.props:
        wrappers = defined(Path("artifacts/gc_wrap.o"))
        missing = {"__wrap_" + s for s in SYMBOLS} - wrappers
        if missing:
            raise SystemExit(f"Adapter symbol mismatch: {sorted(missing)}")
        root = ET.Element("Project")
        items = ET.SubElement(root, "ItemGroup")
        for symbol in SYMBOLS:
            ET.SubElement(items, "LinkerArg", Include="-Wl,--wrap=" + symbol)
        ET.indent(root)
        ET.ElementTree(root).write(args.props, encoding="unicode")
    if args.check_runtime:
        libraries = list(args.check_runtime.glob("libRuntime*.a"))
        if not libraries:
            raise SystemExit(f"No NativeAOT runtime archives in {args.check_runtime}")
        symbols = set().union(*(defined(path) for path in libraries))
        missing = set(SYMBOLS) - symbols
        if missing:
            raise SystemExit(f"Unsupported NativeAOT runtime ABI: {sorted(missing)}")
        print("Pinned NativeAOT GC ABI: all six symbols found")
    if not args.props and not args.check_runtime:
        parser.error("specify --props or --check-runtime")

if __name__ == "__main__":
    main()

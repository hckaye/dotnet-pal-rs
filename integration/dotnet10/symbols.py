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

def audit_runtime(path):
    # Use the exact archive selected by SetupOSSpecificProps, not a guessed
    # NuGet layout. SDK-managed packs can live outside project.assets.json.
    path = path.resolve()
    if path.name != "libRuntime.WorkstationGC.a" or "10.0.0" not in path.parts:
        raise SystemExit(f"Unaudited NativeAOT runtime archive: {path}")
    if not path.is_file():
        raise SystemExit(f"Selected NativeAOT runtime archive is missing: {path}")
    missing = set(SYMBOLS) - defined(path)
    if missing:
        raise SystemExit(f"Unsupported NativeAOT runtime ABI: {sorted(missing)}")
    print(f"Pinned NativeAOT GC ABI: all six symbols found in {path}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--props", type=Path)
    parser.add_argument("--runtime", type=Path, help="exact runtime archive selected by MSBuild")
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
    if args.runtime:
        audit_runtime(args.runtime)
    if not args.props and not args.runtime:
        parser.error("specify --props or --runtime")

if __name__ == "__main__":
    main()

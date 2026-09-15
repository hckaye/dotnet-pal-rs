#!/usr/bin/env python3
"""Verify exact, pinned ELF C++ ABI names; fail instead of silently testing no hooks."""
import argparse
import hashlib
import json
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
RUNTIME_REVISION = "60629d14374c56f1cb51819049ad1fa529307f8d"


def defined(path):
    result = subprocess.run(["nm", "-g", "--defined-only", str(path)],
                            check=True, capture_output=True, text=True)
    return {line.split()[-1] for line in result.stdout.splitlines() if line.split()}


def audit_runtime(path, source_manifest=None):
    path = path.resolve()
    if path.name not in ("libRuntime.WorkstationGC.a", "libRuntime.ServerGC.a"):
        raise SystemExit(f"Unaudited NativeAOT runtime archive: {path}")
    if not path.is_file():
        raise SystemExit(f"Selected NativeAOT runtime archive is missing: {path}")
    if source_manifest is None:
        if "10.0.0" not in path.parts:
            raise SystemExit(f"Unpinned published archive: {path}")
    else:
        try:
            manifest = json.loads(Path(source_manifest).read_text())
        except (OSError, ValueError) as error:
            raise SystemExit(f"Cannot read source manifest: {error}") from error
        if not isinstance(manifest, dict):
            raise SystemExit("Source manifest must be an object")
        if manifest.get("runtime_revision") != RUNTIME_REVISION:
            raise SystemExit("Source manifest has an unaudited runtime revision")
        if manifest.get("adapter") != "dotnet-pal-gc-vm-v2":
            raise SystemExit("Source manifest has an unknown adapter")
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
        digests = manifest.get("archives")
        expected = (digests.get(path.name) if isinstance(digests, dict) else
                    manifest.get("archive_sha256") if path.name == "libRuntime.WorkstationGC.a" else None)
        if expected != digest:
            raise SystemExit("Source archive does not match its build manifest")
    missing = set(SYMBOLS) - defined(path)
    if missing:
        raise SystemExit(f"Unsupported NativeAOT runtime ABI: {sorted(missing)}")
    print(f"Pinned NativeAOT GC ABI: all six symbols found in {path}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--props", type=Path)
    parser.add_argument("--runtime", type=Path, help="exact runtime archive selected by MSBuild")
    parser.add_argument("--source-manifest", type=Path, help="manifest for a source-rebuilt native runtime archive")
    args = parser.parse_args()
    if args.source_manifest and not args.runtime:
        parser.error("--source-manifest requires --runtime")
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
        audit_runtime(args.runtime, args.source_manifest)
    if not args.props and not args.runtime:
        parser.error("specify --props or --runtime")


if __name__ == "__main__":
    main()

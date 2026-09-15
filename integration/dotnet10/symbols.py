#!/usr/bin/env python3
"""Verify exact, pinned ELF C++ ABI names; fail instead of silently testing no hooks."""
import argparse
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

def defined(path):
    result = subprocess.run(["nm", "-g", "--defined-only", str(path)],
                            check=True, capture_output=True, text=True)
    return {line.split()[-1] for line in result.stdout.splitlines() if line.split()}

def runtime_archives(assets, rid):
    data = json.loads(assets.read_text())
    packages = {}
    for name, info in data.get("libraries", {}).items():
        if info.get("type") == "package":
            package, version = name.rsplit("/", 1)
            packages[package.lower()] = version
    # SDK framework/native runtime packs can be PackageDownload entries, not
    # ordinary PackageReferences. Their locations also honor packageFolders.
    for framework in data.get("project", {}).get("frameworks", {}).values():
        for item in framework.get("downloadDependencies", []):
            bounds = item["version"].strip("[]").split(",")
            if len(set(v.strip() for v in bounds)) != 1:
                raise SystemExit("Expected an exact SDK package download version")
            packages[item["name"].lower()] = bounds[0].strip()
    candidates = {name: version for name, version in packages.items()
                  if rid in name and ("nativeaot" in name or "ilcompiler" in name)}
    archives = []
    for name, version in candidates.items():
        if version != "10.0.0":
            raise SystemExit(f"Unaudited NativeAOT pack {name}/{version}; expected 10.0.0")
        for folder in data.get("packageFolders", {}):
            archives.extend((Path(folder) / name / version).rglob("libRuntime.WorkstationGC.a"))
    if len(archives) != 1:
        raise SystemExit(f"Expected one restored WorkstationGC archive; found {archives}; packs={candidates}")
    print(f"Auditing restored NativeAOT runtime: {archives[0]}")
    return archives


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--props", type=Path)
    parser.add_argument("--assets", type=Path, help="restored project.assets.json")
    parser.add_argument("--rid", choices=("linux-x64", "linux-arm64"))
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
    if args.assets:
        if not args.rid:
            parser.error("--assets requires --rid")
        libraries = runtime_archives(args.assets, args.rid)
        symbols = set().union(*(defined(path) for path in libraries))
        missing = set(SYMBOLS) - symbols
        if missing:
            raise SystemExit(f"Unsupported NativeAOT runtime ABI: {sorted(missing)}")
        print("Pinned NativeAOT GC ABI: all six symbols found")
    if not args.props and not args.assets:
        parser.error("specify --props or --assets")

if __name__ == "__main__":
    main()

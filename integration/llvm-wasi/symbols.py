#!/usr/bin/env python3
"""Audit the selected, version-pinned runtime and generate exact Wasm --wrap flags."""
import argparse
from pathlib import Path
import subprocess
import xml.etree.ElementTree as ET

SYMBOLS = (
    '_ZN15GCToOSInterface14VirtualReserveEmmjt',
    '_ZN15GCToOSInterface13VirtualCommitEPvmt',
    '_ZN15GCToOSInterface15VirtualDecommitEPvm',
    '_ZN15GCToOSInterface14VirtualReleaseEPvm',
    '_ZN15GCToOSInterface12VirtualResetEPvmb',
    '_ZN15GCToOSInterface33VirtualReserveAndCommitLargePagesEmt',
    '_ZN15GCToOSInterface23QueryPerformanceCounterEv',
    '_ZN15GCToOSInterface25QueryPerformanceFrequencyEv',
    '_ZN15GCToOSInterface24GetLowPrecisionTimeStampEv',
)

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--runtime', type=Path)
    parser.add_argument('--wrapper', type=Path)
    parser.add_argument('--props', type=Path)
    parser.add_argument('--nm', default='llvm-nm')
    args = parser.parse_args()
    target = args.runtime or args.wrapper
    if target is None: parser.error('provide --runtime or --wrapper')
    if args.runtime and (target.name != 'libPortableRuntime.a' or '10.0.0-rc.1.26357.1' not in target.parts):
        raise SystemExit('unaudited runtime archive path')
    output = subprocess.check_output([args.nm, '--defined-only', str(target)], text=True)
    names = {line.split()[-1] for line in output.splitlines() if line.split()}
    expected = {('__wrap_' if args.wrapper else '') + name for name in SYMBOLS}
    if expected - names: raise SystemExit('missing hooks: ' + str(sorted(expected - names)))
    if args.props:
        root = ET.Element('Project')
        items = ET.SubElement(root, 'ItemGroup')
        for name in SYMBOLS: ET.SubElement(items, 'LinkerArg', Include='-Wl,--wrap=' + name)
        ET.indent(root)
        ET.ElementTree(root).write(args.props, encoding='unicode')
    print('LLVM GC SYMBOL AUDIT PASS: ' + str(target))

if __name__ == '__main__': main()

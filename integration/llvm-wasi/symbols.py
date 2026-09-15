#!/usr/bin/env python3
"""Audit exact LLVM runtime identities and generate Wasm --wrap flags."""
import argparse
import hashlib
import json
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
REVISION = '9954350a58ede8b8eaaeb24112ca4f1e78cc527c'


def verify_identity(target, manifest):
    if target.name != 'libPortableRuntime.a': raise ValueError('unexpected runtime archive name')
    if manifest is None:
        if '10.0.0-rc.1.26357.1' not in target.parts: raise ValueError('unpinned runtime path')
    else:
        data = json.loads(manifest.read_text())
        if not isinstance(data, dict) or data.get('runtime_revision') != REVISION or data.get('adapter') != 'dotnet-pal-llvm-linear-v1':
            raise ValueError('source manifest identity mismatch')
        with target.open('rb') as stream: actual = hashlib.file_digest(stream, 'sha256').hexdigest()
        if data.get('archive_sha256') != actual: raise ValueError('rebuilt runtime digest mismatch')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument('--runtime', type=Path)
    group.add_argument('--wrapper', type=Path)
    parser.add_argument('--source-manifest', type=Path)
    parser.add_argument('--props', type=Path)
    parser.add_argument('--nm', default='llvm-nm')
    args = parser.parse_args()
    if args.source_manifest and not args.runtime: parser.error('source manifest requires runtime')
    target = args.runtime or args.wrapper
    if args.runtime: verify_identity(target.resolve(), args.source_manifest)
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

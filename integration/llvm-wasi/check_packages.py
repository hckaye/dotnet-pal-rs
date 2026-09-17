#!/usr/bin/env python3
"""Verify all three independently restored LLVM packages, including the target pack."""
import base64
import hashlib
import json
import os
from pathlib import Path
import xml.etree.ElementTree as ET

root = Path(__file__).resolve().parents[2]
pins = json.loads((root / 'integration/llvm-wasi/toolchain.json').read_text())
assets = json.loads((root / 'samples/LlvmGcProbe/obj/project.assets.json').read_text())
chosen = {k.lower(): v for k, v in assets['libraries'].items() if 'ilcompiler.llvm/' in k.lower()}
# Exactly one host compiler pack, for the RID the SDK selected (linux-x64, linux-arm64, ...).
hosts = [k for k in chosen if k.startswith('runtime.') and not k.startswith('runtime.wasi-wasm.')]
if len(hosts) != 1:
    raise SystemExit('Expected exactly one host compiler pack: ' + str(sorted(chosen)))
host_rid = hosts[0].split('.')[1]
pinned = dict(pins['packages_sha512'])
if host_rid not in pins['host_packages_sha512']:
    raise SystemExit('Unpinned host compiler pack RID: ' + host_rid)
pinned['runtime.' + host_rid + '.microsoft.dotnet.ilcompiler.llvm'] = pins['host_packages_sha512'][host_rid]
expected = {name + '/' + pins['llvm_version'] for name in pinned}
if set(chosen) != expected:
    raise SystemExit('Unexpected LLVM package set: ' + str(sorted(chosen)))
cache = Path(os.environ['NUGET_PACKAGES'])
report = {'llvm_version': pins['llvm_version'], 'host_rid': host_rid, 'packages': {}}
for name, digest in pinned.items():
    directory = cache / name / pins['llvm_version']
    archive = directory / (name + '.' + pins['llvm_version'] + '.nupkg')
    with archive.open('rb') as stream:
        actual = base64.b64encode(hashlib.file_digest(stream, 'sha512').digest()).decode()
    if actual != digest:
        raise SystemExit('LLVM package integrity mismatch: ' + name)
    nuspec = ET.parse(directory / (name + '.nuspec'))
    repositories = [node.attrib for node in nuspec.iter() if node.tag.split('}')[-1] == 'repository']
    report['packages'][name] = {'sha512': actual, 'repository': repositories}
(root / 'artifacts/llvm/compiler-inputs.json').write_text(json.dumps(report, indent=2) + '\n')
print(json.dumps(report))

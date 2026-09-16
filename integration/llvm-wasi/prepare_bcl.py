#!/usr/bin/env python3
"""Select an audited portable managed cryptography implementation for WASIp1.

The WASI pack ships a platform-not-supported cryptography assembly. The same
upstream build's browser implementation uses System.Native for RNG, and managed
SHA/HMAC. Keep native libraries and CoreLib from WASI; never import browser JS
support as a fabricated host service. Unsupported JS-dependent APIs remain
unsupported and must not be claimed qualified by the RNG workload.
"""
import argparse, base64, hashlib, io, json, os, shutil, urllib.request, zipfile
from pathlib import Path
import xml.etree.ElementTree as ET
VERSION='10.0.0-rc.1.26357.1'
REVISION='9954350a58ede8b8eaaeb24112ca4f1e78cc527c'
PACKAGE='runtime.browser-wasm.microsoft.dotnet.ilcompiler.llvm'
PACKAGE_SHA512='aSmhsGDMsAnMPxT18btcanqanmc0VWTHyQ1lRuJZCtT9ivRu9s0HhG4VWXcXSUs3kDNEY75Ps7DEiI48gnTl5A=='
ASSEMBLY_SHA256='ff9897be7fbeadcced981edecd0368c718e553e32960379805427637d3b972f5'
ASSEMBLY='System.Security.Cryptography.dll'
ROOT=Path(__file__).resolve().parents[2]

def crypto_package(data):
    if base64.b64encode(hashlib.sha512(data).digest()).decode()!=PACKAGE_SHA512: raise ValueError('unrecognized browser package digest')
    with zipfile.ZipFile(io.BytesIO(data)) as z:
        specs=[x for x in z.namelist() if x.endswith('.nuspec')]
        if len(specs)!=1: raise ValueError('invalid package metadata')
        tree=ET.fromstring(z.read(specs[0]))
        repositories=tree.findall('.//{*}repository')
        if len(repositories)!=1 or repositories[0].get('commit')!=REVISION: raise ValueError('unaudited BCL source revision')
        blob=z.read('runtimes/browser-wasm/lib/'+ASSEMBLY)
        if hashlib.sha256(blob).hexdigest()!=ASSEMBLY_SHA256: raise ValueError('BCL digest mismatch')
        return blob

def verify(framework, sdk):
    for root in (framework, sdk):
        p=root/ASSEMBLY
        if not p.is_file() or hashlib.sha256(p.read_bytes()).hexdigest()!=ASSEMBLY_SHA256:
            raise ValueError('unapproved cryptography assembly: '+str(p))
    print('MANAGED BCL SELECTION AUDIT PASS same upstream browser RNG implementation; WASI native platform retained')

def main():
    p=argparse.ArgumentParser();p.add_argument('--check',action='store_true');p.add_argument('--framework',type=Path);p.add_argument('--sdk',type=Path)
    args=p.parse_args()
    if args.check:
        if not args.framework or not args.sdk:p.error('--check requires --framework and --sdk')
        verify(args.framework,args.sdk);return
    packages=Path(os.environ['NUGET_PACKAGES'])
    original=packages/'runtime.wasi-wasm.microsoft.dotnet.ilcompiler.llvm'/VERSION/'runtimes/wasi-wasm'
    if not (original/'lib'/ASSEMBLY).is_file():raise ValueError('restore the pinned WASI compiler pack first')
    out=ROOT/'artifacts/llvm'
    package=out/'browser-bcl.nupkg'
    if not package.exists():
        index=json.load(urllib.request.urlopen('https://pkgs.dev.azure.com/dnceng/public/_packaging/dotnet-experimental/nuget/v3/index.json',timeout=30))
        base=next(r['@id'] for r in index['resources'] if r['@type']=='PackageBaseAddress/3.0.0')
        request=urllib.request.urlopen(f'{base}{PACKAGE}/{VERSION}/{PACKAGE}.{VERSION}.nupkg',timeout=120)
        data=request.read(64*1024*1024+1)
        if len(data)>64*1024*1024:raise ValueError('package exceeds download budget')
        blob=crypto_package(data)
        package.write_bytes(data)
    else:blob=crypto_package(package.read_bytes())
    for folder,source in [('bcl',original/'lib'),('published-sdk',original/'native-hidden')]:
        target=out/folder
        if target.exists():raise ValueError('refusing to replace existing BCL overlay: '+str(target))
        target.mkdir()
        for file in source.iterdir():
            if not file.is_file():raise ValueError('unexpected directory in flat compiler pack')
            if file.name==ASSEMBLY:(target/ASSEMBLY).write_bytes(blob)
            else:(target/file.name).symlink_to(file.resolve())
    (out/'bcl-manifest.json').write_text(json.dumps({'upstream':REVISION,'assembly':ASSEMBLY,'sha256':ASSEMBLY_SHA256,
        'source_package':PACKAGE,'version':VERSION,'scope':'managed RNG and SHA/HMAC implementation; not browser host APIs'},indent=2)+'\n')
    verify(out/'bcl',out/'published-sdk')
if __name__=='__main__':main()

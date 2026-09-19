#!/usr/bin/env python3
"""Exercise real downstream resolution and package-local native bridge builds.

Run from any working directory after the workspace dependencies have been fetched.
Uses offline Cargo resolution; never installs a toolchain or changes the checkout.
"""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parents[1]
CARGO = os.environ.get('CARGO', 'cargo')


def run(*args, cwd=None):
    result = subprocess.run(args, cwd=cwd, text=True, capture_output=True, timeout=180)
    if result.returncode:
        raise RuntimeError(f'{args!r}\n{result.stdout}\n{result.stderr}')
    return result.stdout


def consumer(directory, dependency, package_path, *, features=()):
    directory.mkdir(parents=True, exist_ok=True)
    (directory / 'src').mkdir(exist_ok=True)
    (directory / 'src/lib.rs').write_text('pub fn marker() {}\n')
    (directory / 'Cargo.toml').write_text(f'''[package]
name = "consumer"
version = "0.0.0"
edition = "2021"
[workspace]
[dependencies]
{dependency} = {{ path = {json.dumps(str(package_path))}, features = {json.dumps(list(features))} }}
''')


def graph(directory, target):
    data = json.loads(run(CARGO, 'metadata', '--offline', '--format-version=1', '--filter-platform', target,
                          '--manifest-path', str(directory / 'Cargo.toml')))
    nodes = {n['id']: n for n in data['resolve']['nodes']}
    names = {p['id']: p['name'] for p in data['packages']}
    seen = set()
    def visit(id):
        if id in seen:
            return
        seen.add(id)
        for dep in nodes[id]['dependencies']:
            visit(dep)
    visit(data['resolve']['root'])
    result = {names[id] for id in seen}
    print(target, ':', ', '.join(sorted(result)))
    return result


def listed_files(name):
    names = run(CARGO, 'package', '-p', name, '--list', '--offline', '--allow-dirty', cwd=ROOT).splitlines()
    # Cargo lists its generated manifest/lock metadata as well as source files.
    return [n for n in names if n not in ('.cargo_vcs_info.json', 'Cargo.toml.orig', 'Cargo.lock')]


def main():
    platforms = {'dotnet-pal-linux', 'dotnet-pal-linux-std', 'dotnet-pal-macos', 'dotnet-pal-windows',
                 'dotnet-pal-host', 'dotnet-pal-wasm', 'dotnet-pal-wasip1', 'dotnet-pal-standalone'}
    with tempfile.TemporaryDirectory(prefix='pal-downstream-') as tmp:
        tmp = Path(tmp)
        d = tmp / 'core-only'
        consumer(d, 'dotnet-pal-rs', ROOT)
        for target in ('x86_64-unknown-linux-gnu', 'x86_64-pc-windows-msvc', 'aarch64-apple-darwin', 'wasm32-wasip1', 'thumbv7em-none-eabi'):
            assert graph(d, target) == {'consumer', 'dotnet-pal-rs'}
        # All core features remain platform-free; compile a real no_std native archive.
        with (d / 'Cargo.toml').open('a') as f:
            f.write('[lib]\ncrate-type = ["staticlib"]\n[profile.release]\npanic = "abort"\n')
        (d / 'src/lib.rs').write_text('''#![no_std]
pub struct Clock;
impl dotnet_pal_rs::port::Clock for Clock {
    fn monotonic_ns() -> dotnet_pal_rs::port::Result<u64> { Ok(42) }
}
dotnet_pal_rs::define_pal! { Clock = Clock }
dotnet_pal_rs::define_panic_handler!(dotnet_pal_rs::port::Trap);
''')
        run(CARGO, 'build', '--offline', '--release', '--all-features', '--manifest-path', str(d / 'Cargo.toml'))
        archive = d / 'target/release/libconsumer.a'
        undefined = {line.split()[-1] for line in run('nm', '-u', str(archive)).splitlines() if ' U ' in line}
        for forbidden in ('mmap', 'munmap', 'pthread_create', '__wasi_', 'getrandom', 'socket', 'CreateThread'):
            assert not any(symbol == forbidden or (forbidden == '__wasi_' and symbol.startswith(forbidden)) for symbol in undefined), forbidden
        c = d / 'client.c'
        c.write_text('''#include "dotnet_pal.h"
#include <assert.h>
int main(void) {
    const dotnet_pal_api *api = dotnet_pal_get_api(DOTNET_PAL_ABI_VERSION);
    assert(api && api->header.capabilities == DOTNET_PAL_CAP_CLOCK);
    assert(api->vm.reserve == 0 && api->linear.allocate == 0);
    uint64_t now = 0;
    assert(api->services.monotonic_ns(&now) == DOTNET_PAL_OK && now == 42);
    return 0;
}
''')
        run('cc', '-I', str(ROOT / 'include'), str(c), str(archive), '-Wl,--gc-sections', '-o', str(d / 'client'))
        run(str(d / 'client'))
        print('CORE-ONLY C ABI PASS; no provider dependencies or imports')
        for package in ('dotnet-pal-linux', 'dotnet-pal-linux-std', 'dotnet-pal-host', 'dotnet-pal-storage', 'dotnet-pal-build'):
            d = tmp / package
            consumer(d, package, ROOT / 'crates' / package)
            names = graph(d, 'x86_64-unknown-linux-gnu')
            assert names & platforms == ({package} if package in platforms else set())
            assert 'dotnet-pal-posix' not in names
        d = tmp / 'facade'
        consumer(d, 'dotnet-pal-std', ROOT / 'crates/dotnet-pal-std')
        for target, selected in [('x86_64-unknown-linux-gnu', 'dotnet-pal-linux-std'),
                                 ('aarch64-apple-darwin', 'dotnet-pal-macos'),
                                 ('x86_64-pc-windows-msvc', 'dotnet-pal-windows')]:
            assert graph(d, target) & platforms == {selected}
        assert not any(f.startswith('tests/') for f in listed_files('dotnet-pal-std'))
        for package in ('dotnet-pal-rs', 'dotnet-pal-build', 'dotnet-pal-posix'):
            files = listed_files(package)
            if package == 'dotnet-pal-rs':
                assert all(not f.startswith(('native/', 'crates/', 'tools/', 'examples/', 'integration/')) for f in files)
                assert not any(Path(f).stem.startswith(('linux', 'host', 'standalone', 'storage', 'wasi_p1')) for f in files)
            if package == 'dotnet-pal-build':
                assert not any('posix.c' in f for f in files)
            source = ROOT if package == 'dotnet-pal-rs' else ROOT / 'crates' / package
            dest = tmp / 'relocated' / package
            for file in files:
                src = source / file
                if src.is_file():
                    out = dest / file
                    out.parent.mkdir(parents=True, exist_ok=True)
                    shutil.copyfile(src, out)
            # Resolve not-yet-published workspace packages locally, as path patches
            # would for registry packages. Only package-listed files are copied.
            manifest = dest / 'Cargo.toml'
            text = manifest.read_text()
            if package == 'dotnet-pal-rs':
                begin, end = text.index('[workspace]'), text.index('[lib]')
                text = text[:begin] + '[workspace]\n' + text[end:]
            else:
                text = text.replace('path = "../.."', 'path = "../dotnet-pal-rs"')
                text += '\n[workspace]\n'
            manifest.write_text(text)
        d = tmp / 'packaged-bridge-consumer'
        consumer(d, 'dotnet-pal-rs', tmp / 'relocated/dotnet-pal-rs')
        with (d / 'Cargo.toml').open('a') as f:
            f.write(f'''[build-dependencies]
dotnet-pal-build = {{ path = {json.dumps(str(tmp / 'relocated/dotnet-pal-build'))} }}
[features]
posix = ["dotnet-pal-build/posix"]
''')
        (d / 'build.rs').write_text('''fn main() {
    use dotnet_pal_build::{Adapter, Build};
    assert!(dotnet_pal_build::include_dir().join("dotnet_pal.h").is_file());
    assert!(dotnet_pal_build::source_root().join("native/minipal_entropy_adapter.c").is_file());
    Build::new().adapter(Adapter::MinipalEntropy).compile().expect("packaged bridge");
    let result = Build::new().name("posix_fixture").adapter(Adapter::LinearHeapPosix).compile();
    if cfg!(feature = "posix") { result.expect("explicit POSIX helper"); }
    else { assert!(result.unwrap_err().to_string().contains("explicit posix feature")); }
}
''')
        assert 'DOTNET_PAL_SOURCE_ROOT' not in os.environ, 'unset the source-root override for relocation validation'
        assert 'dotnet-pal-posix' not in graph(d, 'x86_64-unknown-linux-gnu')
        run(CARGO, 'build', '--offline', '--manifest-path', str(d / 'Cargo.toml'))
        run(CARGO, 'build', '--offline', '--features', 'posix', '--manifest-path', str(d / 'Cargo.toml'))
        print('RELOCATED PACKAGE BRIDGE PASS; POSIX absent by default and usable by opt-in')
    print('CRATE BOUNDARIES PASS')


if __name__ == '__main__':
    main()

"""Compile both providers' cache routines with deterministic OS observations.

Only sysconf and file reads are substituted. The method bodies, path builder,
attribute selection and size parsers come from the actual provider sources.
This runs on x64 too, so a sysfs-only (ARM64-like) host is always covered.
"""
from pathlib import Path
import os
import platform
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


def block(text, signature):
    start = text.index(signature)
    opening = text.index('{', start)
    depth = 1
    end = opening + 1
    while depth:
        if text[end] == '{':
            depth += 1
        elif text[end] == '}':
            depth -= 1
        end += 1
    return text[start:end]


FIXTURE = r'''
#![allow(dead_code, unused_unsafe)]
use std::sync::Mutex;
#[derive(Debug)]
enum Error { InvalidArgument, Os }
type Result<T> = std::result::Result<T, Error>;
struct Provider;
struct Observation { sysconf: [i64; 4], files: Vec<(String, String)> }
static OS: Mutex<Observation> = Mutex::new(Observation { sysconf: [0; 4], files: Vec::new() });
mod libc {
    pub const _SC_LEVEL1_DCACHE_SIZE: i32 = 0;
    pub const _SC_LEVEL2_CACHE_SIZE: i32 = 1;
    pub const _SC_LEVEL3_CACHE_SIZE: i32 = 2;
    pub const _SC_LEVEL4_CACHE_SIZE: i32 = 3;
    pub unsafe fn sysconf(name: i32) -> i64 { super::OS.lock().unwrap().sysconf[name as usize] }
}
fn read(path: &str) -> Option<String> {
    OS.lock().unwrap().files.iter().find(|(p, _)| p == path).map(|(_, value)| value.clone())
}
fn read_file(path: &[u8], buffer: &mut [u8]) -> Option<usize> {
    let path = std::ffi::CStr::from_bytes_with_nul(path).unwrap().to_str().unwrap();
    let value = read(path)?;
    let size = buffer.len().min(value.len());
    buffer[..size].copy_from_slice(&value.as_bytes()[..size]);
    Some(size)
}
fn main() {
    // A larger instruction-only cache must never dominate the data-cache
    // summary. A high index is intentional: indices are not cache levels.
    let scenarios: Vec<([i64; 4], Vec<(u32, u32, &str, &str)>, usize)> = vec![
        ([0, 0, 0, 0], vec![(0, 1, "Data", "32K"), (1, 1, "Instruction", "128M"), (2, 2, "Unified", "1M"), (6, 3, "Unified", "32M")], 32 << 20),
        ([32768, 0, 0, 0], vec![(6, 3, "Unified", "64M")], 64 << 20),
        ([32768, 1048576, 33554432, 0], vec![(7, 4, "Unified", "128M")], 128 << 20),
        ([32768, 1048576, 524288, 0], vec![], 1048576),
        ([0, 0, 0, 0], vec![(1, 1, "Instruction", "128M")], 0),
        ([0, 0, 0, 0], vec![], 0),
        ([0, 0, 0, 0], vec![(0, 1, "Data", "bad"), (2, 2, "Unified", "2M")], 2 << 20),
    ];
    for (index, (sysconf, caches, expected)) in scenarios.into_iter().enumerate() {
        let mut os = OS.lock().unwrap();
        os.sysconf = sysconf;
        os.files.clear();
        for (slot, level, kind, size) in caches {
            let prefix = format!("/sys/devices/system/cpu/cpu0/cache/index{slot}");
            for (name, value) in [("level", level.to_string()), ("type", kind.into()), ("size", size.into())] {
                os.files.push((format!("{prefix}/{name}"), value + "\n"));
            }
        }
        drop(os);
        let actual = Provider::cache_size().unwrap();
        assert_eq!(actual, expected, "scenario {index}");
        for level in 1..=4 {
            assert!(actual >= Provider::cache_level_size(level).unwrap(), "summary lost level {level}");
        }
    }
}
'''


@unittest.skipUnless(platform.system() == 'Linux', 'Linux sysconf/sysfs routines')
class CacheSummaryTests(unittest.TestCase):
    def check_provider(self, path, desktop):
        text = (ROOT / path).read_text()
        methods = '\n'.join(block(text, '    fn ' + name) for name in
                            ('cache_size() -> Result<usize>', 'cache_level_size(level: u32) -> Result<usize>'))
        if desktop:
            helpers = block(text, 'fn parse_size(')
        else:
            helpers = '\n'.join(block(text, signature) for signature in
                                ('fn trim(', 'fn parse_u64(', 'fn parse_size(', 'fn starts_with(',
                                 'struct Path ', 'impl Path ', 'fn cache_attribute('))
        rustc = shutil.which(os.environ.get('RUSTC', 'rustc'))
        self.assertIsNotNone(rustc, 'rustc is required')
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source = root / 'cache.rs'
            source.write_text(FIXTURE + '\n' + helpers + '\nimpl Provider {\n' + methods + '\n}\n')
            result = subprocess.run([rustc, '--edition=2021', '-O', str(source), '-o', str(root / 'cache')],
                                    text=True, capture_output=True, timeout=30)
            self.assertEqual(result.returncode, 0, result.stderr)
            result = subprocess.run([str(root / 'cache')], text=True, capture_output=True, timeout=30)
            self.assertEqual(result.returncode, 0, result.stderr)

    def test_no_std_linux_summary(self):
        self.check_provider('crates/dotnet-pal-linux/src/linux_platform.rs', desktop=False)

    def test_std_linux_summary(self):
        self.check_provider('crates/dotnet-pal-linux-std/src/system.rs', desktop=True)

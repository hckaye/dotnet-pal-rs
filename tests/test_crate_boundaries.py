"""Platform choice must never flow backwards into the ABI/contracts crate."""
from pathlib import Path
import re
import tomllib
import unittest

ROOT = Path(__file__).resolve().parents[1]


def manifest(name):
    path = ROOT if name == 'dotnet-pal-rs' else ROOT / 'crates' / name
    return tomllib.loads((path / 'Cargo.toml').read_text())


class CrateBoundaries(unittest.TestCase):
    def test_core_is_always_dependency_free(self):
        m = manifest('dotnet-pal-rs')
        self.assertEqual(m.get('dependencies', {}), {})
        self.assertEqual(m.get('build-dependencies', {}), {})
        self.assertEqual(m.get('target', {}), {})
        self.assertEqual(m['features'], {'default': []})
        self.assertEqual(m['workspace']['default-members'], ['.'])
        for name in ('linux.rs', 'host.rs', 'wasi_p1.rs', 'storage.rs', 'standalone.rs'):
            self.assertFalse((ROOT / 'src' / name).exists(), name)
        self.assertFalse((ROOT / 'build.rs').exists())

    def test_each_desktop_port_owns_its_source(self):
        systems = {'dotnet-pal-linux-std': 'linux', 'dotnet-pal-macos': 'macos', 'dotnet-pal-windows': 'windows'}
        for name, os in systems.items():
            m = manifest(name)
            deps = m.get('dependencies', {})
            self.assertIn('dotnet-pal-rs', deps)
            self.assertFalse(set(systems) & set(deps))
            for source in (ROOT / 'crates' / name / 'src').rglob('*.rs'):
                text = source.read_text()
                self.assertNotRegex(text, r'#\[path\s*=')
                self.assertNotRegex(text, r'include!\([^)]*\.\./')
                for other in set(systems.values()) - {os}:
                    self.assertNotRegex(text, r'#\[cfg\([^\n]*target_os\s*=\s*"' + other + '"')

    def test_facade_is_only_target_selected_reexports(self):
        m = manifest('dotnet-pal-std')
        self.assertFalse(m.get('dependencies', {}))
        text = (ROOT / 'crates/dotnet-pal-std/src/lib.rs').read_text()
        self.assertNotIn('impl ', text)
        self.assertLess(len(text.splitlines()), 40)
        for os, package in [('linux', 'dotnet-pal-linux-std'), ('macos', 'dotnet-pal-macos'), ('windows', 'dotnet-pal-windows')]:
            self.assertEqual(set(m['target'][f'cfg(target_os = "{os}")']['dependencies']), {package})

    def test_bridge_has_only_explicit_posix_reference_dependency(self):
        m = manifest('dotnet-pal-build')
        self.assertEqual(m['features']['default'], [])
        self.assertTrue(m['dependencies']['dotnet-pal-posix']['optional'])
        self.assertEqual(m['features']['posix'], ['dep:dotnet-pal-posix'])
        self.assertFalse((ROOT / 'crates/dotnet-pal-build/native/support_posix.c').exists())
        self.assertTrue((ROOT / 'crates/dotnet-pal-posix/native/support_posix.c').is_file())
        text = (ROOT / 'crates/dotnet-pal-build/src/lib.rs').read_text()
        self.assertNotIn('join("../..")', text)
        self.assertIn('dotnet_pal_rs::C_INCLUDE_DIR', text)

    def test_qualification_assembler_is_not_a_consumer_dependency(self):
        path = ROOT / 'tools/dotnet-pal-standalone/Cargo.toml'
        self.assertFalse(tomllib.loads(path.read_text())['package']['publish'])
        for path in (ROOT / 'crates').glob('*/Cargo.toml'):
            self.assertNotIn('dotnet-pal-standalone', path.read_text())


if __name__ == '__main__':
    unittest.main()

"""Isolation decisions, including real ELF strong/weak references on Linux."""
import importlib.util
import json
from pathlib import Path
import platform
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location('pal_isolation_regression_audit', ROOT / 'scripts/audit_dependencies.py')
audit = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(audit)


def inventory(strong=(), weak=()):
    return {
        'unresolved_strong': [
            {'symbol': name, 'category': audit.classify(name), 'owners': ['runtime.a[probe.o]']}
            for name in strong
        ],
        'unresolved_weak': {name: ['runtime.a[probe.o]'] for name in weak},
    }


class IsolationRegressions(unittest.TestCase):
    ENTRY = 'dotnet_pal_get_api'

    def test_public_entrypoint_only_passes(self):
        self.assertTrue(audit.assess_runtime(inventory([self.ENTRY]))['isolated_runtime'])

    def test_versioned_entrypoint(self):
        self.assertTrue(audit.assess_runtime(inventory([self.ENTRY + '@PAL_2']))['isolated_runtime'])

    def test_empty_archive_fails(self):
        self.assertFalse(audit.assess_runtime(inventory())['isolated_runtime'])

    def test_weak_entrypoint_is_not_integration_evidence(self):
        result = audit.assess_runtime(inventory(weak=[self.ENTRY]))
        self.assertFalse(result['has_runtime_entrypoint'])
        self.assertFalse(result['isolated_runtime'])

    def test_strong_and_weak_os_references_fail(self):
        for binding in ('strong', 'weak'):
            with self.subTest(binding=binding):
                data = inventory([self.ENTRY, 'mmap']) if binding == 'strong' else inventory([self.ENTRY], ['mmap@GLIBC_2.2.5'])
                result = audit.assess_runtime(data)
                self.assertFalse(result['isolated_runtime'])
                self.assertEqual(result['runtime_os_references_outside_boundary'][0]['binding'], binding)

    def test_unknown_weak_reference_fails(self):
        result = audit.assess_runtime(inventory([self.ENTRY], ['vendor_extension']))
        self.assertFalse(result['isolated_runtime'])
        self.assertEqual(result['runtime_unreviewed_references_outside_boundary'][0]['symbol'], 'vendor_extension')

    def test_direct_backend_hooks_fail(self):
        for name in ('dotnet_pal_host_v2', 'dotnet_pal_host_abort', 'dotnet_pal_storage_allocate_v2'):
            with self.subTest(name=name):
                result = audit.assess_runtime(inventory([self.ENTRY, name]))
                self.assertFalse(result['isolated_runtime'])
                self.assertEqual(result['runtime_os_references_outside_boundary'][0]['category'], 'backend-hook-bypasses-runtime-boundary')

    def test_host_hook_is_not_integration_evidence(self):
        self.assertFalse(audit.assess_runtime(inventory(['dotnet_pal_host_v2']))['has_runtime_entrypoint'])

    def test_compiler_prefix_is_not_approval(self):
        self.assertFalse(audit.assess_runtime(inventory([self.ENTRY, '__cxa_unreviewed_extension']))['isolated_runtime'])

    def test_stale_category_is_not_trusted(self):
        data = inventory([self.ENTRY, 'mmap'])
        data['unresolved_strong'][1]['category'] = 'boundary'
        self.assertFalse(audit.assess_runtime(data)['isolated_runtime'])


@unittest.skipUnless(platform.system() == 'Linux', 'ELF fixture requires the Linux CI toolchain')
class RealElfIsolationTests(unittest.TestCase):
    def check_fixture(self, extra, expected, symbol=None, binding=None, entry=True):
        with tempfile.TemporaryDirectory(prefix='pal-isolation-') as directory:
            root = Path(directory)
            code = ('extern void *dotnet_pal_get_api(unsigned);\n'
                    'void *probe(void) { return dotnet_pal_get_api(2); }\n') if entry else ''
            (root / 'runtime.c').write_text(code + extra)
            subprocess.run(['cc', '-fPIC', '-fno-stack-protector', '-c', str(root / 'runtime.c'), '-o', str(root / 'runtime.o')], check=True)
            subprocess.run(['ar', 'rcs', str(root / 'runtime.a'), str(root / 'runtime.o')], check=True)
            subprocess.run(['cc', '-shared', str(root / 'runtime.o'), '-o', str(root / 'probe.so')], check=True)
            output = root / 'report.json'
            command = [sys.executable, str(ROOT / 'scripts/audit_dependencies.py'),
                       '--runtime', str(root / 'runtime.a'), '--pal', str(root / 'runtime.a'),
                       '--binary', str(root / 'probe.so'), '--output', str(output), '--require-isolated']
            run = subprocess.run(command, capture_output=True, text=True, timeout=20)
            self.assertEqual(run.returncode == 0, expected, run.stdout + run.stderr)
            report = json.loads(output.read_text())
            self.assertEqual(report['isolated_runtime'], expected)
            if symbol:
                items = report['runtime_os_references_outside_boundary'] + report['runtime_unreviewed_references_outside_boundary']
                self.assertTrue(any(i['symbol'] == symbol and i['binding'] == binding for i in items), items)

    def test_real_entrypoint_archive(self):
        self.check_fixture('', True)

    def test_real_weak_os_import(self):
        self.check_fixture('extern void *mmap(void) __attribute__((weak));\nvoid *call_os(void) { return mmap ? mmap() : (void *)0; }\n', False, 'mmap', 'weak')

    def test_real_strong_backend_hook(self):
        self.check_fixture('extern void dotnet_pal_host_abort(void);\nvoid bad(void) { dotnet_pal_host_abort(); }\n', False, 'dotnet_pal_host_abort', 'strong')

    def test_real_unreviewed_crt_name(self):
        self.check_fixture('extern void __cxa_unreviewed_extension(void);\nvoid bad(void) { __cxa_unreviewed_extension(); }\n', False, '__cxa_unreviewed_extension', 'strong')

    def test_missing_entrypoint_still_writes_report(self):
        self.check_fixture('void unrelated(void) {}\n', False, entry=False)


if __name__ == '__main__':
    unittest.main()

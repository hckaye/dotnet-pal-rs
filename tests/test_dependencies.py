import importlib.util
from pathlib import Path
import unittest

path = Path(__file__).resolve().parents[1] / 'scripts/audit_dependencies.py'
spec = importlib.util.spec_from_file_location('dependency_inventory', path)
audit = importlib.util.module_from_spec(spec)
spec.loader.exec_module(audit)

class DependencyInventoryTests(unittest.TestCase):
    def test_gnu_and_llvm_owners_and_weak_imports(self):
        rows = audit.parse_nm('lib.a[obj.o]: mmap U\nlib.a(other.o): helper T 0 10\nbinary: __gmon_start__ w\n')
        self.assertEqual(rows, [('lib.a[obj.o]', 'mmap', 'U'), ('lib.a(other.o)', 'helper', 'T'), ('binary', '__gmon_start__', 'w')])
    def test_unknown_format_fails_instead_of_silently_omitting_symbols(self):
        with self.assertRaises(ValueError): audit.parse_nm('mmap (unknown format)')
    def test_signal_thread_and_unknown_calls_remain_visible(self):
        self.assertEqual(audit.classify('pthread_sigmask@GLIBC_2.2.5'), 'signals-and-context')
        self.assertEqual(audit.classify('pthread_create'), 'threads-and-synchronization')
        self.assertEqual(audit.classify('new_platform_syscall'), 'other-needs-review')
        self.assertEqual(audit.classify('dotnet_pal_get_api'), 'boundary')
        self.assertEqual(audit.classify('mmap'), 'memory-and-native-allocator')
    def test_manifest_reviews_non_os_contracts_only(self):
        reviewed = audit.load_reviewed()
        self.assertEqual(audit.reviewed_category('memcpy', ['a.a[x.o]'], reviewed), 'crt-freestanding')
        self.assertEqual(audit.reviewed_category('_ZnwmRKSt9nothrow_t', ['a.a[x.o]'], reviewed), 'cxx-abi')
        self.assertEqual(audit.reviewed_category('RhpNewFast', ['a.a[x.o]'], reviewed), 'managed-contract')
        self.assertEqual(audit.reviewed_category('dst', ['a.a[WriteBarriers.S.o]'], reviewed), 'assembler-artifact')
        self.assertIsNone(audit.reviewed_category('dst', ['a.a[other.o]'], reviewed))
        self.assertIsNone(audit.reviewed_category('sysconf', ['a.a[x.o]'], reviewed))
        self.assertIsNone(audit.reviewed_category('minipal_hires_ticks', ['a.a[x.o]'], reviewed))
    def test_isolation_requires_reviewed_or_boundary_only(self):
        runtime = {'unresolved_strong': [{'symbol': 'dotnet_pal_get_api', 'owners': ['a.a[x.o]']}, {'symbol': 'memcpy', 'owners': ['a.a[x.o]']}], 'unresolved_weak': {}}
        self.assertTrue(audit.assess_runtime(runtime)['isolated_runtime'])
        runtime['unresolved_strong'].append({'symbol': 'new_helper', 'owners': ['a.a[x.o]']})
        report = audit.assess_runtime(runtime)
        self.assertFalse(report['isolated_runtime'])
        self.assertEqual([i['symbol'] for i in report['runtime_unreviewed_references_outside_boundary']], ['new_helper'])
        runtime['unresolved_strong'][-1]['symbol'] = 'sysconf'
        report = audit.assess_runtime(runtime)
        self.assertEqual([i['symbol'] for i in report['runtime_os_references_outside_boundary']], ['sysconf'])
    def test_merge_resolves_references_inside_the_archive_set(self):
        first = {'path': 'a', 'sha256': '1', 'dynamic_symbols_only': False, 'defined': ['helper'], 'tool_warnings': '',
                 'unresolved_strong': [{'symbol': 'minipal_x', 'category': 'other-needs-review', 'owners': ['a[1.o]']}], 'unresolved_weak': {}}
        second = {'path': 'b', 'sha256': '2', 'dynamic_symbols_only': False, 'defined': ['minipal_x'], 'tool_warnings': '',
                  'unresolved_strong': [{'symbol': 'helper', 'category': 'other-needs-review', 'owners': ['b[2.o]']}, {'symbol': 'mmap', 'category': 'memory-and-native-allocator', 'owners': ['b[2.o]']}], 'unresolved_weak': {}}
        merged = audit.merge([first, second])
        self.assertEqual([i['symbol'] for i in merged['unresolved_strong']], ['mmap'])

    def test_vxsort_isa_contract_is_exact_and_owner_scoped(self):
        reviewed = audit.load_reviewed()
        for name in ('_Z25IsSupportedInstructionSet14InstructionSet', '_Z27InitSupportedInstructionSeti'):
            for owner in ('runtime.a[gcwks.cpp.o]', 'runtime.a[gcsvr.cpp.o]'):
                self.assertEqual(audit.reviewed_category(name, [owner], reviewed), 'vxsort-isa-contract')
            self.assertIsNone(audit.reviewed_category(name, ['runtime.a[unknown.cpp.o]'], reviewed))
            self.assertIsNone(audit.reviewed_category(name + '_unexpected', ['runtime.a[gcwks.cpp.o]'], reviewed))
        self.assertIsNone(audit.reviewed_category('__tls_get_addr', ['runtime.a[AllocFast.S.o]'], reviewed))

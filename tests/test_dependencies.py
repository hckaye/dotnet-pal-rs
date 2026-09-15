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

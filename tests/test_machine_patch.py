from pathlib import Path
import sys
import unittest
ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "integration/dotnet10"))
import machine_patch as patch

class MachinePatchTests(unittest.TestCase):
    def test_definition_preserves_disabled_implementation(self):
        text = 'uint64_t query()\n{\n    return sys_value();\n}\n'
        value = patch.function(text, 'query', '    return neutral_value();')
        self.assertIn('#ifdef DOTNET_PAL_MACHINE\n    return neutral_value();\n#else', value)
        self.assertIn('    return sys_value();', value)
        for bad in ('', text + text):
            with self.assertRaises(ValueError): patch.function(bad, 'query', 'return 0;')

    def test_repatches_and_drift_rejected(self):
        for transform in (patch.gc, patch.pal, patch.cgroup):
            with self.assertRaises(ValueError): transform('DOTNET_PAL_MACHINE')
            with self.assertRaises(ValueError): transform('not the pinned source')

    def test_cgroup_policy_not_replaced(self):
        text = '''
    if (getrlimit(RLIMIT_AS, &curr_rlimit) == 0)
    long pages = sysconf(_SC_PHYS_PAGES);
        long pageSize = sysconf(_SC_PAGE_SIZE);
            long pageSize = sysconf(_SC_PAGE_SIZE);
    physical_memory_limit = (physical_memory_limit < rlimit_soft_limit) ?
                            physical_memory_limit : rlimit_soft_limit;
'''
        result = patch.cgroup(text)
        self.assertIn('physical_memory_limit < rlimit_soft_limit',result)
        self.assertEqual(result.count('dotnet_pal_machine::long_query'),2)
        self.assertIn('dotnet_pal_machine::read_limit',result)
        with self.assertRaises(ValueError): patch.cgroup(text.replace('sysconf(_SC_PHYS_PAGES)', 'changed()'))

    def test_public_api_does_not_leak_native_layouts(self):
        text = (ROOT / 'include/dotnet_pal.h').read_text()
        self.assertIn('dotnet_pal_support_ops support;\n    dotnet_pal_machine_ops machine;', text)
        group = text.split('uint32_t (*query)(uint32_t kind, uint64_t *out);',1)[1].split('} dotnet_pal_machine_ops;',1)[0]
        for name in ('cpu_set_t', 'rlimit', 'sysinfo'):
            self.assertNotIn(name, group)

if __name__ == '__main__': unittest.main()

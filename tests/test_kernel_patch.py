import sys
from pathlib import Path
import unittest
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'integration/dotnet10'))
import kernel_patch as patch

class KernelPatchTests(unittest.TestCase):
    def test_function_retains_upstream_branch(self):
        source = 'void GCToOSInterface::FlushProcessWriteBuffers()\n{\n    original();\n}\n'
        result = patch.function(source, 'GCToOSInterface::FlushProcessWriteBuffers', '    replacement();')
        self.assertIn('#ifdef DOTNET_PAL_KERNEL\n    replacement();\n#else\n    original();', result)
        for invalid in ('', source + source):
            with self.assertRaises(ValueError):
                patch.function(invalid, 'GCToOSInterface::FlushProcessWriteBuffers', '    replacement();')

    def test_recursive_gc_lock_is_a_guarded_layout_change(self):
        text = 'class CLRCriticalSection final\n{\n    minipal_mutex m_cs;\n};\n'
        result = patch.header(text)
        self.assertIn('void *m_cs = nullptr', result)
        self.assertIn('dotnet_pal_kernel::mutex_init', result)
        self.assertIn(text.rstrip(), result)
        with self.assertRaises(ValueError): patch.header(text + text)

    def test_crst_rejects_missing_or_duplicate_field(self):
        field = '    minipal_mutex    m_Lock;'
        self.assertIn('void* m_Lock;', patch.crst_header(field))
        for text in ('', field + field):
            with self.assertRaises(ValueError): patch.crst_header(text)

    def test_all_transform_paths_are_explicit_and_unique(self):
        self.assertEqual(set(patch.FILES), set(patch.TRANSFORMS))
        self.assertEqual(len(patch.FILES), 5)
        with self.assertRaises(ValueError): patch.pal('')
        with self.assertRaises(ValueError): patch.events('')

import sys
from pathlib import Path
import unittest
sys.path.insert(0,str(Path(__file__).resolve().parents[1]/'integration/dotnet10'))
import runtime_patch
import kernel_patch

class RuntimePatchTests(unittest.TestCase):
    def test_multiline_signature_keeps_original_and_own_guard(self):
        source='bool Foo(\n    void *value,\n    size_t size)\n{\n    return original(value, size);\n}\n'
        out=runtime_patch.function(source,'Foo','    return routed(value, size);')
        self.assertIn('#ifdef DOTNET_PAL_RUNTIME',out)
        self.assertIn('return original(value, size);',out)
        self.assertIn('return routed(value, size);',out)
        self.assertNotIn('DOTNET_PAL_KERNEL',out)
        for bad in ('',source+source):
            with self.assertRaises(ValueError): runtime_patch.function(bad,'Foo','return false;')

    def test_servicing_state_declaration_outside_removed_barrier_helpers(self):
        source=Path(__file__).resolve().parents[1]/'integration/dotnet10/kernel_patch.py'
        text=source.read_text()
        self.assertIn('g_flushProcessWriteBuffersMutex;',text)
        self.assertNotIn('end = text.index("size_t GetRestrictedPhysicalMemoryLimit();", start)',text)

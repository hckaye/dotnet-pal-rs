from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[1]

class NativeAotEnvironmentContractTests(unittest.TestCase):
    def test_hex_only_gc_limits_in_execution_scripts(self):
        for name in ('nativeaot.sh', 'qualify.sh', 'source-runtime.sh'):
            text = (ROOT / 'scripts' / name).read_text()
            self.assertNotRegex(text, r'DOTNET_GCHeapHardLimit=0[xX]')
            for value in re.findall(r'DOTNET_GCHeapHardLimit=([a-zA-Z0-9]+)', text):
                self.assertRegex(value, r'^[0-9a-fA-F]+$')
                self.assertGreater(int(value, 16), 0)

    def test_qualification_observes_effective_heap_limit(self):
        text = (ROOT / 'samples/GcProbe/HeapLimitPreflight.cs').read_text()
        self.assertIn('GC.GetGCMemoryInfo().TotalAvailableMemoryBytes', text)
        self.assertIn('(ulong)actual != expected', text)

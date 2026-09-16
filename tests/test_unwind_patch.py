import sys, unittest
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'integration/dotnet10'))
import unwind_patch
class UnwindPatchTests(unittest.TestCase):
    def test_unique_and_guarded(self):
        source='namespace libunwind {\n#if defined(_LIBUNWIND_HAS_NO_THREADS)\n\nclass'
        result=unwind_patch.transform(source)
        self.assertIn('using RWMutex = DotnetPalUnwindLock;', result)
        self.assertIn('#elif defined(_LIBUNWIND_HAS_NO_THREADS)', result)
        with self.assertRaises(ValueError): unwind_patch.transform(result)
        with self.assertRaises(ValueError): unwind_patch.transform(source+source)
    def test_sparse_checkout_and_input_guard(self):
        root=Path(__file__).resolve().parents[1]
        self.assertIn('/'+unwind_patch.FILE,(root/'.github/workflows/ci.yml').read_text())
        self.assertIn('unwind_patch.FILE]',(root/'integration/dotnet10/patch_runtime.py').read_text())

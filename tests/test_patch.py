import importlib.util
from pathlib import Path
import unittest

path = Path(__file__).resolve().parents[1] / "integration/dotnet10/patch_runtime.py"
spec = importlib.util.spec_from_file_location("patch_runtime", path)
patch = importlib.util.module_from_spec(spec)
spec.loader.exec_module(patch)

class PatchTests(unittest.TestCase):
    def fixture(self):
        return "\n".join(f"bool GCToOSInterface::{name}(void)\n{{\n    return false;\n}}\n" for name in patch.CALLS)

    def test_six_guarded_replacements(self):
        result = patch.patch_gc(self.fixture())
        self.assertEqual(result.count("#else"), 6)
        self.assertEqual(result.count("return dotnet_pal_gc::"), 6)
        self.assertEqual(result.count("return false;"), 6)

    def test_refuse_partial_or_duplicate_input(self):
        with self.assertRaises(ValueError):
            patch.patch_gc("")
        with self.assertRaises(ValueError):
            patch.patch_gc(self.fixture() + self.fixture())

    def test_refuse_double_patch(self):
        with self.assertRaises(ValueError):
            patch.patch_gc(patch.patch_gc(self.fixture()))

if __name__ == "__main__":
    unittest.main()

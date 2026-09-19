"""Qualification scripts must refer to shipped bridge assets after relocation."""
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[1]


class BridgeAssetPaths(unittest.TestCase):
    def test_integration_sources_referenced_by_scripts_exist(self):
        references = set()
        for source in (ROOT / "scripts").glob("*.sh"):
            text = source.read_text()
            for relative in re.findall(r"crates/dotnet-pal-build/[A-Za-z0-9_./-]+\.(?:cpp|c)\b", text):
                references.add(relative)
                self.assertTrue((ROOT / relative).is_file(), f"{source.name}: {relative}")
        for relative in (
            "integration/dotnet10/gc_wrap.cpp",
            "integration/llvm-wasi/gc_linear_wrap.cpp",
            "integration/llvm-wasi/p1_error_text.c",
        ):
            self.assertIn("crates/dotnet-pal-build/" + relative, references)


if __name__ == "__main__":
    unittest.main()

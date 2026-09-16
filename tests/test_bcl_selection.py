import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
path=Path(__file__).resolve().parents[1]/'integration/llvm-wasi/prepare_bcl.py'
spec=importlib.util.spec_from_file_location('bcl_selection',path)
bcl=importlib.util.module_from_spec(spec);spec.loader.exec_module(bcl)
class BclSelectionTests(unittest.TestCase):
    def test_untrusted_package_is_rejected_before_zip_parsing(self):
        for data in (b'',b'not a package'):
            with self.assertRaises(ValueError): bcl.crypto_package(data)
    def test_both_compiler_resolution_paths_are_checked(self):
        import hashlib
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);a=root/'framework';b=root/'sdk';a.mkdir();b.mkdir()
            good=b'tested assembly';digest=hashlib.sha256(good).hexdigest()
            (a/bcl.ASSEMBLY).write_bytes(good);(b/bcl.ASSEMBLY).write_bytes(good)
            with patch.object(bcl,'ASSEMBLY_SHA256',digest):
                bcl.verify(a,b)
                (b/bcl.ASSEMBLY).write_bytes(b'wrong selected native-hidden assembly')
                with self.assertRaises(ValueError):bcl.verify(a,b)

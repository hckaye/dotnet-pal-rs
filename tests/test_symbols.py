import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

path = Path(__file__).resolve().parents[1] / "integration/dotnet10/symbols.py"
spec = importlib.util.spec_from_file_location("symbols", path)
symbols = importlib.util.module_from_spec(spec)
spec.loader.exec_module(symbols)

class ArchiveTests(unittest.TestCase):
    def test_exact_sdk_link_input(self):
        with tempfile.TemporaryDirectory() as directory:
            archive = Path(directory) / "sdk-pack" / "10.0.0" / "native/libRuntime.WorkstationGC.a"
            archive.parent.mkdir(parents=True)
            archive.touch()
            with patch.object(symbols, "defined", return_value=set(symbols.SYMBOLS)):
                symbols.audit_runtime(archive)
            with patch.object(symbols, "defined", return_value=set(symbols.SYMBOLS[1:])):
                with self.assertRaises(SystemExit):
                    symbols.audit_runtime(archive)

    def test_wrong_version_or_missing_archive(self):
        with self.assertRaises(SystemExit):
            symbols.audit_runtime(Path("10.0.1/libRuntime.WorkstationGC.a"))
        with self.assertRaises(SystemExit):
            symbols.audit_runtime(Path("missing/10.0.0/libRuntime.WorkstationGC.a"))

import hashlib
import importlib.util
import json
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
            archive = Path(directory) / "sdk-pack" / "10.0.12" / "native/libRuntime.WorkstationGC.a"
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
            symbols.audit_runtime(Path("missing/10.0.12/libRuntime.WorkstationGC.a"))

class SourceManifestTests(unittest.TestCase):
    def test_source_archive_must_match_manifest(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            archive = root / 'libRuntime.WorkstationGC.a'
            archive.write_bytes(b'test archive')
            manifest_path = root / 'manifest.json'
            manifest = {'runtime_revision': symbols.RUNTIME_REVISION,
                        'adapter': 'dotnet-pal-gc-vm-v2',
                        'archive_sha256': hashlib.sha256(archive.read_bytes()).hexdigest()}
            manifest_path.write_text(json.dumps(manifest))
            with patch.object(symbols, 'defined', return_value=set(symbols.SYMBOLS)):
                symbols.audit_runtime(archive, manifest_path)
                with self.assertRaises(SystemExit):
                    symbols.audit_runtime(archive)
                archive.write_bytes(b'stale archive')
                with self.assertRaises(SystemExit):
                    symbols.audit_runtime(archive, manifest_path)
                manifest['runtime_revision'] = 'unreviewed'
                manifest_path.write_text(json.dumps(manifest))
                with self.assertRaises(SystemExit):
                    symbols.audit_runtime(archive, manifest_path)

    def test_missing_or_malformed_manifest(self):
        with tempfile.TemporaryDirectory() as directory:
            archive = Path(directory) / 'libRuntime.WorkstationGC.a'
            archive.write_bytes(b'test')
            manifest = Path(directory) / 'manifest.json'
            with self.assertRaises(SystemExit):
                symbols.audit_runtime(archive, manifest)
            for data in ('not json', '[]', '{}'):
                manifest.write_text(data)
                with self.subTest(data=data), self.assertRaises(SystemExit):
                    symbols.audit_runtime(archive, manifest)

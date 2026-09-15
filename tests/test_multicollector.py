import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

path = Path(__file__).resolve().parents[1] / 'integration/dotnet10/symbols.py'
spec = importlib.util.spec_from_file_location('symbols', path)
symbols = importlib.util.module_from_spec(spec)
spec.loader.exec_module(symbols)

class MultiCollectorManifestTests(unittest.TestCase):
    def test_server_and_workstation_digests_are_not_interchangeable(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            paths = [root / ('libRuntime.' + name + 'GC.a') for name in ('Workstation', 'Server')]
            for index, file in enumerate(paths):
                file.write_bytes(bytes([index]))
            manifest = {'runtime_revision': symbols.RUNTIME_REVISION,
                        'adapter': 'dotnet-pal-gc-vm-v2',
                        'archives': {f.name: hashlib.sha256(f.read_bytes()).hexdigest() for f in paths}}
            file = root / 'manifest.json'
            file.write_text(json.dumps(manifest))
            with patch.object(symbols, 'defined', return_value=set(symbols.SYMBOLS)):
                for archive in paths:
                    symbols.audit_runtime(archive, file)
                paths[1].write_bytes(paths[0].read_bytes())
                with self.assertRaises(SystemExit):
                    symbols.audit_runtime(paths[1], file)

    def test_legacy_digest_cannot_authorize_server_gc(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            archive = root / 'libRuntime.ServerGC.a'
            archive.write_bytes(b'archive')
            manifest = root / 'manifest.json'
            manifest.write_text(json.dumps({'runtime_revision': symbols.RUNTIME_REVISION,
                'adapter': 'dotnet-pal-gc-vm-v2',
                'archive_sha256': hashlib.sha256(archive.read_bytes()).hexdigest()}))
            with self.assertRaises(SystemExit):
                symbols.audit_runtime(archive, manifest)

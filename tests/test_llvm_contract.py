import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
def load(name, file):
    spec = importlib.util.spec_from_file_location(name, ROOT / 'integration/llvm-wasi' / file)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module

class LlvmContractTests(unittest.TestCase):
    def test_rewrite_is_exact_and_rejects_missing_or_duplicate_definitions(self):
        patch = load('llvm_patch', 'patch_runtime.py')
        calls = patch.VM_CALLS
        source = '\n'.join('bool GCToOSInterface::' + name + '(void)\n{\n    return false;\n}' for name in calls)
        changed = patch.replace_definitions(source, calls)
        self.assertEqual(changed.count('return dotnet_pal_gc_linear::'), 6)
        with self.assertRaises(ValueError): patch.replace_definitions('', calls)
        with self.assertRaises(ValueError): patch.replace_definitions(source + '\n' + source, calls)

    def test_source_identity_rejects_corruption_and_wrong_revision(self):
        symbols = load('llvm_symbols', 'symbols.py')
        with tempfile.TemporaryDirectory() as directory:
            archive = Path(directory) / 'libPortableRuntime.a'
            archive.write_bytes(b'compiled runtime')
            manifest = Path(directory) / 'manifest.json'
            payload = {'runtime_revision': symbols.REVISION, 'adapter': 'dotnet-pal-llvm-linear-v1',
                       'archive_sha256': hashlib.sha256(archive.read_bytes()).hexdigest()}
            manifest.write_text(json.dumps(payload))
            symbols.verify_identity(archive, manifest)
            archive.write_bytes(b'other runtime')
            with self.assertRaises(ValueError): symbols.verify_identity(archive, manifest)
            with self.assertRaises(ValueError): symbols.verify_identity(archive, None)
            payload['runtime_revision'] = 'unreviewed'
            manifest.write_text(json.dumps(payload))
            with self.assertRaises(ValueError): symbols.verify_identity(archive, manifest)

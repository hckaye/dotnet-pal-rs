import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

path = Path(__file__).resolve().parents[1] / "integration/dotnet10/symbols.py"
spec = importlib.util.spec_from_file_location("symbols", path)
symbols = importlib.util.module_from_spec(spec)
spec.loader.exec_module(symbols)

class PackageTests(unittest.TestCase):
    def test_sdk_package_download_and_custom_cache(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            name = "Microsoft.NETCore.App.Runtime.NativeAOT.linux-arm64"
            archive = root / "cache" / name.lower() / "10.0.0" / "runtimes/linux-arm64/native/sdk/libRuntime.WorkstationGC.a"
            archive.parent.mkdir(parents=True)
            archive.touch()
            assets = root / "project.assets.json"
            assets.write_text(json.dumps({
                "packageFolders": {str(root / "cache"): {}},
                "project": {"frameworks": {"net10.0": {"downloadDependencies": [
                    {"name": name, "version": "[10.0.0, 10.0.0]"}
                ]}}}
            }))
            self.assertEqual(symbols.runtime_archives(assets, "linux-arm64"), [archive])

    def test_wrong_version_is_not_silently_accepted(self):
        with tempfile.TemporaryDirectory() as directory:
            assets = Path(directory) / "project.assets.json"
            assets.write_text(json.dumps({"libraries": {
                "runtime.linux-x64.microsoft.dotnet.ilcompiler/10.0.1": {"type": "package"}
            }}))
            with self.assertRaises(SystemExit):
                symbols.runtime_archives(assets, "linux-x64")

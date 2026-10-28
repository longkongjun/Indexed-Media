import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('check_vendor', Path(__file__).with_name('check-vendor.py'))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class VendorTest(unittest.TestCase):
    def test_real_pin_and_tampered_artifact(self):
        import shutil
        root = Path(__file__).resolve().parents[2]
        module.validate(root)
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory)
            shutil.copytree(root / 'vendor/indexed-design', target / 'vendor/indexed-design')
            manifest = json.loads((target / 'vendor/indexed-design/desktop-provenance.json').read_text())
            artifact = target / 'vendor/indexed-design/maven' / next(iter(manifest['files']))
            artifact.write_bytes(b'tampered')
            with self.assertRaisesRegex(ValueError, 'digest mismatch'):
                module.validate(target)

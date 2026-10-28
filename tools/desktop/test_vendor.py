import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('check_vendor', Path(__file__).with_name('check-vendor.py'))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class VendorTest(unittest.TestCase):
    def test_tv_pin_rejects_an_unexpected_artifact(self):
        import shutil
        root = Path(__file__).resolve().parents[2]
        module.validate(root, 'tv')
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory)
            shutil.copytree(root / 'vendor/indexed-design', target / 'vendor/indexed-design')
            manifest_path = target / 'vendor/indexed-design/tv-provenance.json'
            manifest = json.loads(manifest_path.read_text())
            manifest['files']['../../escape'] = '0' * 64
            manifest_path.write_text(json.dumps(manifest))
            with self.assertRaisesRegex(ValueError, 'artifact set changed'):
                module.validate(target, 'tv')

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

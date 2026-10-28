import importlib.util
import json
import shutil
import subprocess
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('check_vendor', Path(__file__).with_name('check-vendor.py'))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class VendorTest(unittest.TestCase):
    def test_windows_checkout_preserves_pinned_artifact_bytes(self):
        root = Path(__file__).resolve().parents[2]
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory)
            shutil.copytree(root / 'vendor/indexed-design', target / 'vendor/indexed-design')
            attributes = root / '.gitattributes'
            if attributes.exists():
                shutil.copy(attributes, target / attributes.name)

            def git(*args):
                return subprocess.run(['git', '-C', str(target), *args], check=True, capture_output=True)

            git('init')
            git('config', 'core.autocrlf', 'true')
            git('config', 'core.eol', 'crlf')
            git('config', 'core.safecrlf', 'false')
            git('add', 'vendor')
            if attributes.exists():
                git('add', '.gitattributes')
            checkout = target / 'checkout'
            git('checkout-index', '--all', '--force', f'--prefix={checkout.as_posix()}/')
            module.validate(checkout)
            module.validate(checkout, 'tv')

    def test_metadata_line_ending_tampering_is_rejected(self):
        root = Path(__file__).resolve().parents[2]
        for platform in ('desktop', 'tv'):
            with self.subTest(platform=platform), tempfile.TemporaryDirectory() as directory:
                target = Path(directory)
                shutil.copytree(root / 'vendor/indexed-design', target / 'vendor/indexed-design')
                manifest = json.loads((target / f'vendor/indexed-design/{platform}-provenance.json').read_text())
                pom = next(path for path in manifest['files'] if path.endswith('.pom'))
                artifact = target / 'vendor/indexed-design/maven' / pom
                artifact.write_bytes(artifact.read_bytes().replace(b'\n', b'\r\n'))
                with self.assertRaisesRegex(ValueError, 'digest mismatch'):
                    module.validate(target, platform)

    def test_tv_pin_rejects_an_unexpected_artifact(self):
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

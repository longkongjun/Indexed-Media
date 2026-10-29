#!/usr/bin/env python3
"""Generate fixed neutral MediaFlow host resources from pinned selected geometry."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[2]
SOURCE = ROOT / 'vendor/indexed-design/brand/media.svg'
VECTOR = ROOT / 'vendor/indexed-design/brand/media-foreground.xml'
MANIFEST = ROOT / 'vendor/indexed-design/brand/native-assets.json'
BACKGROUND = '#242326'
SELECTED_MEDIA_SHA256 = 'b396d8861c798701599f3bc7be5a7d2f9d02d5ecd5d79a617fffa6fe5756d851'

SELECTED_VECTOR_SHA256 = 'dacf6089c4a1e4560c1af6a27b609fa873f240891f706cd99cc8e192fe84d802'


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def generate():
    from PIL import Image
    assert sha(SOURCE) == SELECTED_MEDIA_SHA256, "selected media geometry changed"
    assert sha(VECTOR) == SELECTED_VECTOR_SHA256, "selected platform geometry changed"
    raw = SOURCE.read_text()
    shape = raw[raw.index('>') + 1:raw.rindex('</svg>')].replace('currentColor', '#FFFFFF')
    files = []

    def write(path, text):
        target = ROOT / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(text)
        files.append(path)

    def svg(width, height, fit=.6):
        side = min(width, height) * fit
        return f'<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}"><rect width="100%" height="100%" fill="{BACKGROUND}"/><g transform="translate({(width-side)/2} {(height-side)/2}) scale({side/96})">{shape}</g></svg>'

    def png(path, text):
        target = ROOT / path
        target.parent.mkdir(parents=True, exist_ok=True)
        subprocess.run(['rsvg-convert', '-o', str(target)], input=text.encode(), check=True)
        files.append(path)

    foreground = VECTOR.read_text()
    for app in ['android', 'android-tv']:
        res = f'apps/{app}/src/main/res'
        write(res + '/drawable/brand_foreground.xml', foreground)
        write(res + '/drawable/brand_background.xml', f'<shape xmlns:android="http://schemas.android.com/apk/res/android" android:shape="rectangle"><solid android:color="{BACKGROUND}"/></shape>\n')
        write(res + '/drawable/brand_icon.xml', foreground.replace('<group ', f'<path android:fillColor="{BACKGROUND}" android:pathData="M0 0H108V108H0Z"/><group ', 1))
        for api, mono in [(26, ''), (33, '<monochrome android:drawable="@drawable/brand_foreground"/>')]:
            write(res + f'/mipmap-anydpi-v{api}/ic_launcher.xml', '<adaptive-icon xmlns:android="http://schemas.android.com/apk/res/android"><background android:drawable="@drawable/brand_background"/><foreground android:drawable="@drawable/brand_foreground"/>' + mono + '</adaptive-icon>\n')
    banner = svg(320, 180, .56).replace('</svg>', '<text x="160" y="164" text-anchor="middle" font-family="sans-serif" font-size="19" fill="#FFFFFF">MediaFlow</text></svg>')
    png('apps/android-tv/src/main/res/drawable-nodpi/brand_banner.png', banner)
    png('apps/desktop/src/main/resources/brand/icon.png', svg(256, 256))
    png('apps/desktop/icons/icon-1024.png', svg(1024, 1024))
    with Image.open(ROOT / 'apps/desktop/icons/icon-1024.png') as image:
        for ext in ['ico', 'icns']:
            path = f'apps/desktop/icons/icon.{ext}'
            image.save(ROOT / path, format=ext.upper(), **({'sizes': [(32, 32), (48, 48), (64, 64), (128, 128), (256, 256)]} if ext == 'ico' else {}))
            files.append(path)
    data = {'schema_version': 1, 'source_commit': '9d0038c16688a4a74058b3b8f0d7899d4aadbbdb', 'symbol': 'media', 'appearance': 'fixed neutral, independent of supported themes and default theme', 'background': BACKGROUND, 'source_sha256': sha(SOURCE), 'vector_sha256': sha(VECTOR), 'generator_sha256': sha(Path(__file__)), 'files': {p: sha(ROOT / p) for p in sorted(files)}, 'limits': ['Platform masks, installed appearance, packaging, remote control and human acceptance pending. Android 12+ uses the launcher icon for its system splash; no custom motion.']}
    MANIFEST.write_text(json.dumps(data, indent=2) + '\n')


def check():
    data = json.loads(MANIFEST.read_text())
    assert sha(SOURCE) == data['source_sha256'] == SELECTED_MEDIA_SHA256
    assert sha(VECTOR) == data['vector_sha256'] == SELECTED_VECTOR_SHA256
    assert sha(Path(__file__)) == data['generator_sha256']
    for path, digest in data['files'].items():
        assert sha(ROOT / path) == digest, path
        if path.endswith('.xml'):
            ET.parse(ROOT / path)
        if path.endswith('.png'):
            import struct
            raw = (ROOT / path).read_bytes()
            assert raw[:8] == b'\x89PNG\r\n\x1a\n'
            width, height, depth, color = struct.unpack('>IIBB', raw[16:26])
            assert depth == 8 and color == 2, 'opaque RGB required'
            if 'banner' in path:
                assert (width, height) == (320, 180)
        if path.endswith('.ico'):
            assert (ROOT / path).read_bytes()[:4] == b'\x00\x00\x01\x00'
        if path.endswith('.icns'):
            assert (ROOT / path).read_bytes()[:4] == b'icns'
    assert data['symbol'] == 'media'
    print(f"PASS MediaFlow native assets: {len(data['files'])} files, selected media geometry, neutral host resources")


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--check', action='store_true')
    if not parser.parse_args().check:
        generate()
    check()

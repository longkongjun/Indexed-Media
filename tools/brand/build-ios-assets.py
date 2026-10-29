#!/usr/bin/env python3
"""Build fixed-neutral iOS launch resources from the pinned selected MediaFlow SVG."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[2]
SOURCE = ROOT / 'vendor/indexed-design/brand/media.svg'
MANIFEST = ROOT / 'vendor/indexed-design/ios-brand-provenance.json'
ASSETS = 'apps/ios/MediaFlowiOS/Assets.xcassets'
BACKGROUND = '#242326'


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def generate():
    raw = SOURCE.read_text()
    shape = raw[raw.index('>') + 1:raw.rindex('</svg>')].replace('currentColor', '#FFFFFF')
    files = []

    def write(path, text):
        target = ROOT / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(text)
        files.append(path)

    def png(path, svg):
        target = ROOT / path
        target.parent.mkdir(parents=True, exist_ok=True)
        subprocess.run(['rsvg-convert', '-o', str(target)], input=svg.encode(), check=True)
        files.append(path)

    info = {'author': 'xcode', 'version': 1}
    write(ASSETS + '/Contents.json', json.dumps({'info': info}, indent=2) + '\n')
    icon = '<svg xmlns="http://www.w3.org/2000/svg" width="1024" height="1024"><rect width="100%" height="100%" fill="' + BACKGROUND + '"/><g transform="translate(204.8 204.8) scale(6.4)">' + shape + '</g></svg>'
    png(ASSETS + '/AppIcon.appiconset/icon-1024.png', icon)
    write(ASSETS + '/AppIcon.appiconset/Contents.json', json.dumps({'images': [{'filename': 'icon-1024.png', 'idiom': 'universal', 'platform': 'ios', 'size': '1024x1024'}], 'info': info}, indent=2) + '\n')
    mark = '<svg xmlns="http://www.w3.org/2000/svg" width="288" height="288" viewBox="0 0 96 96">' + shape + '</svg>'
    png(ASSETS + '/LaunchMark.imageset/media.png', mark)
    write(ASSETS + '/LaunchMark.imageset/Contents.json', json.dumps({'images': [{'filename': 'media.png', 'idiom': 'universal', 'scale': '3x'}], 'info': info}, indent=2) + '\n')
    data = {'schema_version': 1, 'source_commit': '9d0038c16688a4a74058b3b8f0d7899d4aadbbdb', 'symbol': 'media', 'appearance': 'fixed neutral #242326/white, independent of default theme', 'source_sha256': sha(SOURCE), 'generator_sha256': sha(Path(__file__)), 'files': {p: sha(ROOT / p) for p in files}, 'limits': ['Source assets and Xcode resource compilation do not establish installed mask, physical device, VoiceOver, Dynamic Type or human visual acceptance.']}
    MANIFEST.write_text(json.dumps(data, indent=2) + '\n')


def check():
    data = json.loads(MANIFEST.read_text())
    assert sha(SOURCE) == data['source_sha256']
    assert sha(Path(__file__)) == data['generator_sha256']
    for path, digest in data['files'].items():
        assert sha(ROOT / path) == digest, path
    vendor = ROOT / 'vendor/indexed-design'
    for path, digest in json.loads((vendor / 'apple-provenance.json').read_text())['files'].items():
        assert sha(vendor / 'apple' / path) == digest, path
    print(f"PASS iOS resources: {len(data['files'])} hashes and pinned SwiftPM source")


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--check', action='store_true')
    if not parser.parse_args().check:
        generate()
    check()

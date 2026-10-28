#!/usr/bin/env python3
"""Verify the pinned unpublished Design library before compiling a consumer."""
import hashlib
import json
from pathlib import Path
import re
import sys


def validate(root: Path, platform: str = 'desktop') -> None:
    if platform not in ('desktop', 'tv'):
        raise ValueError('unsupported platform')
    vendor = root / 'vendor/indexed-design'
    manifest = json.loads((vendor / f'{platform}-provenance.json').read_text())
    if manifest.get('schema_version') != 1 or manifest.get('coordinate') != f'dev.indexed.design:compose-{platform}:0.15.0':
        raise ValueError('unexpected schema or coordinate')
    if not re.fullmatch(r'[0-9a-f]{40}', manifest['source_commit']):
        raise ValueError('source commit missing')
    prefix = f'dev/indexed/design/compose-{platform}/0.15.0/compose-{platform}-0.15.0'
    suffixes = ('.jar', '.pom', '.module') if platform == 'desktop' else ('.aar', '.pom', '.module', '-sources.jar')
    if set(manifest['files']) != {prefix + suffix for suffix in suffixes}:
        raise ValueError('artifact set changed')
    for path, digest in manifest['files'].items():
        if hashlib.sha256((vendor / 'maven' / path).read_bytes()).hexdigest() != digest:
            raise ValueError(f'digest mismatch: {path}')


if __name__ == '__main__':
    platform = sys.argv[1] if len(sys.argv) == 2 else 'desktop'
    if len(sys.argv) > 2:
        raise ValueError('expected at most one platform')
    validate(Path(__file__).resolve().parents[2], platform)
    print(f'{platform} Design vendor: artifacts verified')

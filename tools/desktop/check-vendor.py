#!/usr/bin/env python3
"""Verify the pinned unpublished desktop library before compiling a consumer."""
import hashlib
import json
from pathlib import Path
import re


def validate(root: Path) -> None:
    vendor = root / 'vendor/indexed-design'
    manifest = json.loads((vendor / 'desktop-provenance.json').read_text())
    if manifest.get('schema_version') != 1 or manifest.get('coordinate') != 'dev.indexed.design:compose-desktop:0.15.0':
        raise ValueError('unexpected schema or coordinate')
    if not re.fullmatch(r'[0-9a-f]{40}', manifest['source_commit']):
        raise ValueError('source commit missing')
    prefix = 'dev/indexed/design/compose-desktop/0.15.0/compose-desktop-0.15.0.'
    if set(manifest['files']) != {prefix + ext for ext in ('jar', 'pom', 'module')}:
        raise ValueError('artifact set changed')
    for path, digest in manifest['files'].items():
        if hashlib.sha256((vendor / 'maven' / path).read_bytes()).hexdigest() != digest:
            raise ValueError(f'digest mismatch: {path}')


if __name__ == '__main__':
    validate(Path(__file__).resolve().parents[2])
    print('Desktop Design vendor: 3 artifacts verified')

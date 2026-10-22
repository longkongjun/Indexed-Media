"""Check actual Web relative-import closure against the Docker Web-stage COPY map."""
import json
from pathlib import Path
import re
import shlex
import unittest

ROOT = Path(__file__).resolve().parents[3]
DOCKERFILE = ROOT / 'infra/containers/Dockerfile'


def copied_paths(dockerfile):
    copies = []
    in_web = False
    for line in dockerfile.splitlines():
        if line.startswith('FROM '):
            if in_web:
                break
            in_web = line.endswith(' AS web-build')
        if in_web and line.startswith('COPY '):
            words = shlex.split(line)[1:]
            copies.extend((ROOT / source, Path(words[-1])) for source in words[:-1])
    return copies


def missing_inputs(dockerfile):
    copies = copied_paths(dockerfile)
    included = json.loads((ROOT / 'apps/web/tsconfig.json').read_text())['include']
    queue = [p for pattern in included for p in (ROOT / 'apps/web').glob(pattern) if p.is_file()]
    visited = set()
    missing = []
    while queue:
        source = queue.pop()
        if source in visited:
            continue
        visited.add(source)
        if source.suffix not in ('.ts', '.tsx', '.js', '.mjs', '.vue'):
            continue
        imports = re.findall(r'''(?:from\s*|import\s*\(\s*|import\s*)["'](\.[^"']+)["']''', source.read_text())
        for specifier in imports:
            base = (source.parent / specifier).resolve()
            target = next((p for p in (base, base.with_suffix('.ts'), base / 'index.ts') if p.is_file()), None)
            if target is None:
                continue  # vue-tsc diagnoses unresolved source imports; this guard covers missing COPY inputs.
            relative = target.relative_to(ROOT)
            covered = False
            for copied, destination in copies:
                if copied.is_dir() and target.is_relative_to(copied):
                    covered |= destination / target.relative_to(copied) == relative
                elif copied == target:
                    final = destination / target.name if str(destination) == '.' else destination
                    covered |= final == relative
            if not covered:
                missing.append(f'{source.relative_to(ROOT)}: {specifier} -> {relative}')
            queue.append(target)
    return missing


class WebBuildInputsTests(unittest.TestCase):
    def test_web_typecheck_import_closure_is_copied(self):
        missing = missing_inputs(DOCKERFILE.read_text())
        if missing:
            self.fail(f'{len(missing)} missing Docker Web inputs:\n' + '\n'.join(missing[:8]))

    def test_missing_direct_and_transitive_inputs_are_detected(self):
        dockerfile = DOCKERFILE.read_text()
        for source in ('packages/test-fixtures', 'packages/ui-contract/contract', 'contracts/examples'):
            broken = '\n'.join(line for line in dockerfile.splitlines() if not line.startswith(f'COPY {source} '))
            with self.subTest(source=source):
                self.assertTrue(any(source in item for item in missing_inputs(broken)))


if __name__ == '__main__':
    unittest.main()

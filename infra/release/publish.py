#!/usr/bin/env python3
"""Produce trusted main candidates and promote their unchanged digest to a release."""
import argparse
import hashlib
import io
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import zipfile

REPO = 'longkongjun/Indexed-Media'
DEV = 'ghcr.io/longkongjun/mediaflow-dev'
PROD = 'ghcr.io/longkongjun/mediaflow'
ROOT = Path(__file__).resolve().parents[2]
FILES = ('release.json', 'compose.yaml', 'env.example', 'roots.example.json', 'install.py', 'release-notes.md')


def require(condition, message):
    if not condition:
        raise ValueError(message)


def command(*args):
    return subprocess.check_output(args, text=True, timeout=900).strip()


def api(path):
    return json.loads(command('gh', 'api', '--hostname', 'github.com', path))


def version(tag):
    require(re.fullmatch(r'v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)', tag), 'expected vX.Y.Z')
    return tag[1:]


def check_run(run, sha):
    require(re.fullmatch('[0-9a-f]{40}', sha), 'invalid source SHA')
    expected = dict(head_sha=sha, head_branch='main', event='push', status='completed',
                    conclusion='success', path='.github/workflows/ci.yml')
    require(all(run.get(k) == v for k, v in expected.items()), 'candidate requires successful trusted main CI')
    require(run.get('repository', {}).get('full_name') == REPO, 'wrong candidate repository')


def proof_for(sha, digest, run_id):
    require(re.fullmatch('[0-9a-f]{40}', sha), 'invalid source SHA')
    require(re.fullmatch('sha256:[0-9a-f]{64}', digest), 'invalid registry digest')
    require(type(run_id) is int and run_id > 0, 'invalid CI run')
    return dict(schema=1, repository=REPO, workflow='.github/workflows/ci.yml', event='push',
                source_sha=sha, digest=digest, image=f'{DEV}:sha-{sha}', platform='linux/amd64', run_id=run_id)


def check_proof(proof, run, sha):
    check_run(run, sha)
    expected = proof_for(sha, proof.get('digest', ''), run['id'])
    require(proof == expected, 'candidate proof does not match trusted run/image/platform')


def remote_digest(ref):
    result = subprocess.run(['docker', 'buildx', 'imagetools', 'inspect', ref, '--format', '{{.Manifest.Digest}}'],
                            text=True, capture_output=True, timeout=120)
    if result.returncode:
        error = result.stderr.lower()
        if ('not found' in error or 'manifest unknown' in error) and not any(
                s in error for s in ('unauthorized', 'denied', 'forbidden', 'timeout')):
            return None
        raise ValueError('registry inspection failed; check authentication/network, do not assume absence')
    digest = result.stdout.strip()
    require(re.fullmatch('sha256:[0-9a-f]{64}', digest), 'registry returned invalid digest')
    return digest


def check_existing(existing, digest):
    require(existing is None or existing == digest, 'immutable version already has a different digest')


def copy_image(source, target, digest):
    existing = remote_digest(target)
    check_existing(existing, digest)
    if existing is None:
        command('docker', 'buildx', 'imagetools', 'create', '--prefer-index=false', '--tag', target, source)
    require(remote_digest(target) == digest, 'promotion changed registry digest')


def check_image(ref, sha):
    command('docker', 'pull', '--platform', 'linux/amd64', ref)
    image = json.loads(command('docker', 'image', 'inspect', ref))[0]
    check_image_identity(image, sha)


def check_image_identity(image, sha):
    require(image.get('Architecture') == 'amd64' and image.get('Os') == 'linux', 'wrong image platform')
    labels = image.get('Config', {}).get('Labels') or {}
    require(labels.get('org.opencontainers.image.revision') == sha, 'wrong OCI revision')
    require(labels.get('org.opencontainers.image.source') == f'https://github.com/{REPO}', 'wrong OCI source')
    require(labels.get('org.opencontainers.image.version') == f'git-{sha}', 'wrong OCI build version')


def private_package(name, allow_missing=False):
    result = subprocess.run(['gh', 'api', f'/users/longkongjun/packages/container/{name}'],
                            capture_output=True, text=True, timeout=60)
    if result.returncode:
        require(allow_missing and '(HTTP 404)' in result.stderr, 'cannot verify private package visibility')
        return
    require(json.loads(result.stdout).get('visibility') == 'private', 'package must remain private')


def candidate(output):
    require(os.environ.get('GITHUB_REPOSITORY') == REPO and os.environ.get('GITHUB_EVENT_NAME') == 'push'
            and os.environ.get('GITHUB_REF') == 'refs/heads/main', 'only trusted main push may publish')
    sha = os.environ['GITHUB_SHA']
    run_id = int(os.environ['GITHUB_RUN_ID'])
    proof_for(sha, 'sha256:' + '0' * 64, run_id)
    private_package('mediaflow-dev', allow_missing=True)
    local = f'mediaflow:sha-{sha}'
    check_image_local = json.loads(command('docker', 'image', 'inspect', local))[0]
    check_image_identity(check_image_local, sha)
    temporary = f'{DEV}:build-{run_id}-{int(os.environ["GITHUB_RUN_ATTEMPT"])}'
    command('docker', 'tag', local, temporary)
    command('docker', 'push', temporary)
    digest = remote_digest(temporary)
    require(digest is not None, 'pushed image missing')
    copy_image(f'{DEV}@{digest}', f'{DEV}:sha-{sha}', digest)
    private_package('mediaflow-dev')
    proof = proof_for(sha, digest, run_id)
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(proof, indent=2) + '\n')
    # The caller holds the shared candidate publish lock while re-reading main.
    head = api(f'/repos/{REPO}/git/ref/heads/main')['object']['sha']
    if head == sha:
        command('docker', 'buildx', 'imagetools', 'create', '--prefer-index=false', '--tag', f'{DEV}:edge', f'{DEV}@{digest}')
        require(remote_digest(f'{DEV}:edge') == digest, 'edge digest mismatch')


def trusted_candidate(sha):
    runs = api(f'/repos/{REPO}/actions/workflows/ci.yml/runs?event=push&head_sha={sha}&per_page=100')['workflow_runs']
    for run in runs:
        try:
            check_run(run, sha)
        except ValueError:
            continue
        artifacts = api(f'/repos/{REPO}/actions/runs/{run["id"]}/artifacts?per_page=100')['artifacts']
        name = f'candidate-proof-{sha}-{run.get("run_attempt", 1)}'
        matches = [a for a in artifacts if a['name'] == name and not a['expired']]
        if not matches:
            continue
        artifact = max(matches, key=lambda a: a['id'])
        require(artifact['size_in_bytes'] < 100_000, 'oversized candidate proof')
        archive = subprocess.check_output(['gh', 'api', f'/repos/{REPO}/actions/artifacts/{artifact["id"]}/zip'], timeout=120)
        with zipfile.ZipFile(io.BytesIO(archive)) as zipped:
            require(zipped.namelist() == ['candidate.json'], 'unexpected candidate artifact files')
            require(zipped.getinfo('candidate.json').file_size < 20_000, 'oversized candidate JSON')
            proof = json.loads(zipped.read('candidate.json'))
        check_proof(proof, run, sha)
        require(remote_digest(proof['image']) == proof['digest'], 'candidate tag/digest mismatch')
        return proof, run
    raise ValueError('no retained trusted candidate; rerun full main CI before release')


def migration_digest(root):
    entries = [(p.name, hashlib.sha256(p.read_bytes()).hexdigest()) for p in sorted((root / 'apps/core/migrations').glob('*.sql'))]
    require(bool(entries), 'missing migrations')
    return hashlib.sha256(json.dumps(entries, separators=(',', ':')).encode()).hexdigest()


def bundle(output, tag, sha, proof, run):
    number = version(tag)
    check_proof(proof, run, sha)
    output.mkdir(parents=True, exist_ok=False)
    metadata = dict(schema=1, version=number, source_sha=sha, platform='linux/amd64',
                    repository=REPO, candidate=proof['image'], image=f'{PROD}@{proof["digest"]}',
                    digest=proof['digest'], ci_run=run['id'], deployment_contract=1,
                    migrations_sha256=migration_digest(ROOT), compatible_from=[], recovery='manual-required',
                    install_mode='first-install-only')
    (output / 'release.json').write_text(json.dumps(metadata, indent=2) + '\n')
    for source, target in [('infra/nas/compose.release.yaml', 'compose.yaml'),
                           ('infra/nas/install.example.json', 'env.example'),
                           ('infra/nas/roots.release.json', 'roots.example.json'),
                           ('infra/nas/install.py', 'install.py'), ('docs/operations/release-notes.md', 'release-notes.md')]:
        shutil.copyfile(ROOT / source, output / target)
    (output / 'checksums.txt').write_text(''.join(f'{hashlib.sha256((output / f).read_bytes()).hexdigest()}  {f}\n' for f in FILES))


def promote(tag, output):
    number = version(tag)
    sha = command('git', 'rev-parse', f'{tag}^{{commit}}')
    command('git', 'merge-base', '--is-ancestor', sha, 'origin/main')
    require(command('git', 'rev-parse', 'HEAD') == sha, 'release checkout must equal tag commit')
    proof, run = trusted_candidate(sha)
    private_package('mediaflow', allow_missing=True)
    copy_image(f'{DEV}@{proof["digest"]}', f'{PROD}:{number}', proof['digest'])
    private_package('mediaflow')
    check_image(f'{PROD}@{proof["digest"]}', sha)
    bundle(output, tag, sha, proof, run)


def release(tag, output):
    number = version(tag)
    manifest = json.loads((output / 'release.json').read_text())
    require(manifest['version'] == number and manifest['repository'] == REPO, 'wrong release bundle')
    # Re-read every bundle checksum before granting publication to this artifact.
    import importlib.util
    spec = importlib.util.spec_from_file_location('installer', ROOT / 'infra/nas/install.py')
    installer = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(installer)
    installer.load_bundle(output, hashlib.sha256((output / 'checksums.txt').read_bytes()).hexdigest())
    require(api(f'/repos/{REPO}/commits/{tag}').get('sha') == manifest['source_sha'], 'tag and bundle source differ')
    existing = subprocess.run(['gh', 'api', f'/repos/{REPO}/releases/tags/{tag}'], capture_output=True, text=True, timeout=60)
    if existing.returncode:
        require('(HTTP 404)' in existing.stderr, 'cannot inspect existing Release')
        command('gh', 'release', 'create', tag, '--repo', REPO, '--verify-tag', '--draft', '--title', tag,
                '--notes-file', str(output / 'release-notes.md'))
        record = api(f'/repos/{REPO}/releases/tags/{tag}')
    else:
        record = json.loads(existing.stdout)
    require(not record.get('prerelease'), 'prerelease is not an installable version')
    assets = {a['name']: a for a in record['assets']}
    for name in (*FILES, 'checksums.txt'):
        expected = 'sha256:' + hashlib.sha256((output / name).read_bytes()).hexdigest()
        if name in assets:
            require(assets[name].get('digest') == expected and assets[name]['state'] == 'uploaded', 'existing Release asset differs')
        else:
            require(record['draft'], 'published Release is incomplete; do not mutate it')
            command('gh', 'release', 'upload', tag, str(output / name), '--repo', REPO)
    final = api(f'/repos/{REPO}/releases/tags/{tag}')
    assets = {a['name']: a for a in final['assets']}
    require(set(assets) == set(FILES) | {'checksums.txt'}, 'unexpected/missing release assets')
    for name, asset in assets.items():
        require(asset.get('digest') == 'sha256:' + hashlib.sha256((output / name).read_bytes()).hexdigest()
                and asset['state'] == 'uploaded', 'uploaded asset mismatch')
    if final['draft']:
        command('gh', 'release', 'edit', tag, '--repo', REPO, '--draft=false')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('mode', choices=['candidate', 'promote', 'release'])
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--tag')
    args = parser.parse_args()
    if args.mode == 'candidate':
        candidate(args.output)
    elif args.mode == 'promote':
        promote(args.tag or '', args.output)
    else:
        release(args.tag or '', args.output)


if __name__ == '__main__':
    try:
        main()
    except (ValueError, KeyError, OSError, subprocess.SubprocessError, zipfile.BadZipFile) as error:
        raise SystemExit(f'release failed: {error}') from None

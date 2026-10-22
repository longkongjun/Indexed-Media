#!/usr/bin/env python3
"""Validate a trusted release bundle and install one NEW Linux amd64 instance."""
import argparse
import fcntl
import hashlib
import ipaddress
import json
import os
from pathlib import Path
import platform
import re
import secrets
import shutil
import socket
import stat
import subprocess
import tempfile
from urllib.parse import urlsplit

REPO = 'longkongjun/Indexed-Media'
FILES = {'release.json', 'compose.yaml', 'env.example', 'roots.example.json', 'install.py', 'release-notes.md'}


def require(condition, message):
    if not condition:
        raise ValueError(message)


def regular(path):
    require(path.is_file() and not path.is_symlink() and path.stat().st_nlink == 1, f'expected independent regular file: {path.name}')


def load_bundle(directory, trusted_checksum):
    directory = Path(directory)
    checksum = directory / 'checksums.txt'
    regular(checksum)
    require(re.fullmatch('[0-9a-f]{64}', trusted_checksum), 'supply the checksums.txt SHA-256 from the trusted published Release')
    require(hashlib.sha256(checksum.read_bytes()).hexdigest() == trusted_checksum, 'untrusted release checksum file')
    records = {}
    for line in checksum.read_text().splitlines():
        match = re.fullmatch(r'([0-9a-f]{64})  ([a-z][a-z.\-]*)', line)
        require(match is not None, 'invalid checksum entry')
        digest, name = match.groups()
        require(name not in records and name in FILES, 'unexpected/duplicate release file')
        records[name] = digest
    require(set(records) == FILES, 'incomplete release bundle')
    for name, digest in records.items():
        path = directory / name
        regular(path)
        require(hashlib.sha256(path.read_bytes()).hexdigest() == digest, f'release file mismatch: {name}')
    data = json.loads((directory / 'release.json').read_text())
    require(isinstance(data, dict) and data.get('schema') == 1 and data.get('repository') == REPO, 'wrong release source/schema')
    require(re.fullmatch(r'(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)', data.get('version', '')), 'wrong release version')
    require(re.fullmatch('[0-9a-f]{40}', data.get('source_sha', '')), 'invalid source SHA')
    require(re.fullmatch('sha256:[0-9a-f]{64}', data.get('digest', '')), 'invalid image digest')
    require(data.get('image') == 'ghcr.io/longkongjun/mediaflow@' + data['digest'], 'only the fixed official digest is installable')
    require(data.get('candidate') == 'ghcr.io/longkongjun/mediaflow-dev:sha-' + data['source_sha'], 'wrong candidate source')
    require(data.get('platform') == 'linux/amd64' and data.get('deployment_contract') == 1, 'unsupported platform/deployment contract')
    require(data.get('install_mode') == 'first-install-only' and data.get('compatible_from') == []
            and data.get('recovery') == 'manual-required', 'this installer does not upgrade or restore')
    require(type(data.get('ci_run')) is int and data['ci_run'] > 0
            and re.fullmatch('[0-9a-f]{64}', data.get('migrations_sha256', '')), 'missing release provenance')
    return data


def safe_path(value):
    require(isinstance(value, str) and value.startswith('/') and not any(ord(c) < 32 or ord(c) == 127 or c in '$:' for c in value),
            'paths must be absolute and contain no interpolation/control characters')
    path = Path(value)
    require('..' not in path.parts and len(path.parts) >= 3, 'unsafe root path')
    for part in (path, *path.parents):
        require(not part.is_symlink(), 'symlink path is forbidden')
    return path


def validate_config(config):
    required = {'instance_dir', 'incoming_dir', 'media_dir', 'uid', 'gid', 'public_origin', 'trusted_proxy_cidrs'}
    require(isinstance(config, dict) and required <= set(config) <= required | {'port', 'bind_address'}, 'missing/unknown installation fields')
    result = dict(config, port=config.get('port', 3000), bind_address=config.get('bind_address', '127.0.0.1'))
    paths = [safe_path(config[k]) for k in ('instance_dir', 'incoming_dir', 'media_dir')]
    instance, incoming, media = paths
    require(not instance.exists() and instance.parent.is_dir(), 'instance must be a NEW directory under an existing parent; existing instances need controlled upgrade')
    require(incoming.is_dir() and media.is_dir(), 'create dedicated incoming and media directories first')
    for a in paths:
        for b in paths:
            if a != b:
                require(a not in b.parents, 'instance and media roots must not overlap')
    require(len(set(paths)) == 3, 'installation roots must be distinct')
    for key in ('uid', 'gid'):
        require(type(result[key]) is int and 0 < result[key] < 2**31, 'non-root numeric UID/GID required')
    require(type(result['port']) is int and 1024 <= result['port'] <= 65535, 'port must be 1024..65535')
    address = ipaddress.ip_address(result['bind_address'])
    require(address.version == 4 and address.is_private and not address.is_unspecified and not address.is_multicast, 'bind to explicit loopback/private IPv4')
    require(isinstance(result['public_origin'], str) and isinstance(result['trusted_proxy_cidrs'], str), 'origin and CIDRs must be strings')
    origin = urlsplit(result['public_origin'])
    require(origin.port is None or 1 <= origin.port <= 65535, 'invalid HTTPS port')
    require(origin.scheme == 'https' and origin.hostname and not origin.username and not origin.password
            and not origin.query and not origin.fragment and origin.path in ('', '/')
            and not any(c in result['public_origin'] for c in ('\n', '\r', '$')), 'trusted HTTPS origin without credentials/path required')
    networks = result['trusted_proxy_cidrs'].split(',')
    require(networks and all(ipaddress.ip_network(n, strict=False).prefixlen > 0 for n in networks), 'explicit proxy CIDRs required; default route is forbidden')
    return result


def run(*args):
    # Compose must consume the reviewed env file, never ambient deployment overrides.
    env = {key: value for key, value in os.environ.items()
           if not key.startswith(("MEDIAFLOW_", "COMPOSE_")) and key not in {"PUID", "PGID", "TZ"}}
    return subprocess.check_output(args, stderr=subprocess.STDOUT, text=True, timeout=900, env=env)


def write_private(path, contents, uid=None, gid=None):
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, 'w') as stream:
        if uid is not None:
            os.fchown(stream.fileno(), uid, gid)
        stream.write(contents)
        stream.flush()
        os.fsync(stream.fileno())


def perform(bundle, trusted_checksum, config, execute=False):
    manifest = load_bundle(bundle, trusted_checksum)
    config = validate_config(config)
    instance = Path(config['instance_dir'])
    project = 'mediaflow-' + hashlib.sha256(str(instance).encode()).hexdigest()[:12]
    plan = dict(version=manifest['version'], image=manifest['image'], platform=manifest['platform'],
                instance_dir=str(instance), incoming_read_only=config['incoming_dir'], media_writable=config['media_dir'],
                uid=config['uid'], gid=config['gid'], project=project, mode='first-install-only',
                runtime_preflight='not-run', acceptance='HTTPS initialization/login/SSE and NAS checks still required')
    if not execute:
        return plan
    require(os.geteuid() == 0 or (config['uid'] == os.getuid() and config['gid'] == os.getgid()), 'use the requested runtime owner or an administrator for newly created ownership')
    require(platform.system() == 'Linux' and platform.machine() in ('x86_64', 'amd64'), 'installation requires Linux amd64 NAS')
    lock = instance.parent / f'.{project}.install.lock'
    fd = os.open(lock, os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, 'w') as locked:
        require(os.fstat(locked.fileno()).st_nlink == 1 and os.fstat(locked.fileno()).st_uid == os.geteuid(), 'unsafe instance lock')
        try:
            fcntl.flock(locked, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            raise ValueError('another installer holds this instance lock') from None
        validate_config(config)
        try:
            run('docker', 'info')
            run('docker', 'compose', 'version')
            require(not run('docker', 'ps', '-aq', '--filter', f'label=com.docker.compose.project={project}').strip(), 'existing Compose instance must not be replaced')
            run('docker', 'pull', '--platform', 'linux/amd64', manifest['image'])
            image = json.loads(run('docker', 'image', 'inspect', manifest['image']))[0]
            require(image.get('Architecture') == 'amd64' and image.get('Os') == 'linux'
                    and manifest['image'] in image.get('RepoDigests', []), 'pulled image digest/platform mismatch')
            labels = image.get('Config', {}).get('Labels') or {}
            require(labels.get('org.opencontainers.image.revision') == manifest['source_sha']
                    and labels.get('org.opencontainers.image.source') == f'https://github.com/{REPO}'
                    and labels.get('org.opencontainers.image.version') == 'git-' + manifest['source_sha'], 'OCI source identity mismatch')
            require(shutil.disk_usage(instance.parent).free >= max(1024**3, image.get('Size', 0) * 2), 'insufficient instance filesystem space')
            run('docker', 'run', '--rm', '--network', 'none', '--read-only', '--cap-drop', 'ALL',
                '--security-opt', 'no-new-privileges:true', '--user', f'{config["uid"]}:{config["gid"]}',
                '--volume', f'{config["incoming_dir"]}:/incoming:ro', '--volume', f'{config["media_dir"]}:/media:rw',
                '--entrypoint', '/bin/sh', manifest['image'], '-ec', 'test -r /incoming && test -x /incoming && test -w /media && test -x /media')
            with socket.socket() as probe:
                probe.bind((config['bind_address'], config['port']))
        except (ValueError, OSError, subprocess.SubprocessError) as error:
            report_dir = Path(tempfile.mkdtemp(prefix=project + '-preflight-'))
            diagnostic = error.output if isinstance(error, subprocess.CalledProcessError) else str(error)
            report_path = report_dir / 'preflight.log'
            write_private(report_path, diagnostic or 'preflight failed')
            raise ValueError(f'preflight failed; preflight report: {report_path}') from None
        # Ownership changes are limited to files/directories created by this invocation.
        instance.mkdir(mode=0o700)
        report = dict(plan, state='installing')
        try:
            for name in ('config', 'secrets'):
                path = instance / name
                path.mkdir(mode=0o700)
                if name == 'config':
                    os.chown(path, config['uid'], config['gid'])
            for name in ('compose.yaml', 'roots.example.json', 'release.json', 'checksums.txt'):
                target = 'deployment-roots.json' if name == 'roots.example.json' else name
                write_private(instance / target, (Path(bundle) / name).read_text())
            # The app must read the read-only roots bind as its configured non-root user.
            os.chmod(instance / 'deployment-roots.json', 0o644)
            write_private(instance / 'secrets/bootstrap.secret', secrets.token_hex(32) + '\n', config['uid'], config['gid'])
            env = dict(MEDIAFLOW_IMAGE=manifest['image'], PUID=config['uid'], PGID=config['gid'],
                       MEDIAFLOW_BIND_ADDRESS=config['bind_address'], MEDIAFLOW_PORT=config['port'],
                       MEDIAFLOW_CONFIG_PATH=str(instance / 'config'), MEDIAFLOW_INCOMING_PATH=config['incoming_dir'],
                       MEDIAFLOW_MEDIA_PATH=config['media_dir'], MEDIAFLOW_ROOTS_CONFIG_PATH=str(instance / 'deployment-roots.json'),
                       MEDIAFLOW_BOOTSTRAP_SECRET_PATH=str(instance / 'secrets/bootstrap.secret'),
                       MEDIAFLOW_PUBLIC_ORIGIN=config['public_origin'], MEDIAFLOW_TRUSTED_PROXY_CIDRS=config['trusted_proxy_cidrs'], TZ='Asia/Shanghai')
            write_private(instance / '.env', ''.join(f'{k}={json.dumps(str(v), ensure_ascii=False)}\n' for k, v in env.items()))
            compose = ['docker', 'compose', '--project-name', project, '--env-file', str(instance / '.env'), '-f', str(instance / 'compose.yaml')]
            rendered = json.loads(run(*compose, 'config', '--format', 'json'))
            service = rendered.get('services', {}).get('mediaflow', {})
            require(set(rendered.get('services', {})) == {'mediaflow'} and service.get('image') == manifest['image']
                    and service.get('user') == f'{config["uid"]}:{config["gid"]}' and service.get('read_only')
                    and not service.get('privileged') and not service.get('network_mode')
                    and 'ALL' in service.get('cap_drop', []), 'unsafe rendered Compose model')
            expected_mounts = {
                '/config': (str(instance / 'config'), False),
                '/data/incoming': (config['incoming_dir'], True),
                '/data/media': (config['media_dir'], False),
                '/run/mediaflow/deployment-roots.json': (str(instance / 'deployment-roots.json'), True),
                '/run/secrets/mediaflow-bootstrap': (str(instance / 'secrets/bootstrap.secret'), True),
            }
            volumes = service.get('volumes', [])
            require(len(volumes) == len(expected_mounts) and all(v.get('type') == 'bind' for v in volumes)
                    and {v.get('target'): (v.get('source'), v.get('read_only', False)) for v in volumes} == expected_mounts,
                    'unexpected rendered Compose mounts')
            output = run(*compose, 'up', '--detach', '--wait', '--wait-timeout', '120', '--pull', 'never')
            write_private(instance / 'install.log', output)
            run(*compose, 'exec', '-T', 'mediaflow', '/usr/local/bin/mediaflow-core', 'healthcheck')
            report.update(state='ready-for-initialization', runtime_preflight='passed')
        except Exception as error:
            report.update(state='failed', error_type=type(error).__name__)
            diagnostic = error.output if isinstance(error, subprocess.CalledProcessError) else str(error)
            write_private(instance / 'failure.log', diagnostic or 'installation failed')
            if not (instance / 'install.log').exists():
                write_private(instance / 'install.log', diagnostic or 'installation failed')
            raise
        finally:
            write_private(instance / 'deployment.json', json.dumps(report, indent=2) + '\n')
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('mode', choices=['check', 'plan', 'install'])
    parser.add_argument('--bundle', type=Path, required=True)
    parser.add_argument('--checksums-sha256', required=True)
    parser.add_argument('--config', type=Path, required=True)
    args = parser.parse_args()
    report = perform(args.bundle, args.checksums_sha256, json.loads(args.config.read_text()), args.mode == 'install')
    print(json.dumps(report, indent=2))


if __name__ == '__main__':
    try:
        main()
    except (ValueError, KeyError, OSError, subprocess.SubprocessError) as error:
        # Detailed subprocess diagnostics stay in the protected instance report.
        detail = str(error) if isinstance(error, ValueError) else type(error).__name__
        raise SystemExit(f'first installation failed: {detail}; inspect configuration or protected install.log') from None

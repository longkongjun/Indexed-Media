import hashlib
import importlib.util
import json
import os
import tempfile
import unittest
from pathlib import Path

SPEC = importlib.util.spec_from_file_location('install', Path(__file__).parents[1] / 'install.py')
install = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(install)


class InstallFixture:
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        for name in ['incoming', 'media', 'bundle']:
            (self.root / name).mkdir()
        self.config = dict(instance_dir=str(self.root / 'instance'), incoming_dir=str(self.root / 'incoming'),
                           media_dir=str(self.root / 'media'), uid=max(os.getuid(), 1000), gid=max(os.getgid(), 1000),
                           public_origin='https://media.example.test', trusted_proxy_cidrs='172.20.0.2/32')



class InstallTests(InstallFixture, unittest.TestCase):
    def test_existing_instance_is_refused_without_mutation(self):
        instance = self.root / 'instance'
        instance.mkdir()
        (instance / 'database.sqlite').write_bytes(b'keep')
        with self.assertRaises(ValueError):
            install.validate_config(self.config)
        self.assertEqual((instance / 'database.sqlite').read_bytes(), b'keep')

    def test_symlink_and_overlapping_media_roots_are_refused(self):
        (self.root / 'link').symlink_to(self.root / 'media', target_is_directory=True)
        for changed in [dict(media_dir=str(self.root / 'link')), dict(media_dir=str(self.root / 'incoming')),
                        dict(instance_dir=str(self.root / 'media' / 'instance'))]:
            with self.subTest(changed=changed), self.assertRaises(ValueError):
                install.validate_config(dict(self.config, **changed))

    def test_no_root_user_or_untrusted_origin_or_compose_interpolation(self):
        for changed in [dict(uid=0), dict(gid=0), dict(uid=True), dict(public_origin='http://example.test'),
                        dict(public_origin='https://user:password@example.test'), dict(incoming_dir='/tmp/$HOME'),
                        dict(trusted_proxy_cidrs='0.0.0.0/0')]:
            with self.subTest(changed=changed), self.assertRaises(ValueError):
                install.validate_config(dict(self.config, **changed))

    def test_control_characters_in_path_are_refused(self):
        with self.assertRaises(ValueError):
            install.validate_config(dict(self.config, instance_dir=str(self.root / "instance\x1b")))

    def test_compose_process_cannot_inherit_deployment_overrides(self):
        from unittest.mock import patch
        with patch.dict(os.environ, {"MEDIAFLOW_MEDIA_PATH": "/unexpected", "COMPOSE_FILE": "/wrong.yml", "PUID": "0"}):
            with patch.object(install.subprocess, "check_output", return_value="") as call:
                install.run("docker", "compose", "config")
                env = call.call_args.kwargs["env"]
                self.assertFalse(set(env) & {"MEDIAFLOW_MEDIA_PATH", "COMPOSE_FILE", "PUID"})

    def test_unknown_config_keys_are_not_silently_ignored(self):
        with self.assertRaises(ValueError):
            install.validate_config(dict(self.config, instal_dir='/wrong'))

    def test_empty_new_instance_can_be_previewed(self):
        result = install.validate_config(self.config)
        self.assertEqual(result['instance_dir'], str(self.root / 'instance'))
        self.assertFalse((self.root / 'instance').exists())

    def test_checksum_file_must_match_independently_supplied_digest(self):
        bundle = self.root / 'bundle'
        (bundle / 'checksums.txt').write_text('not trusted')
        with self.assertRaises(ValueError):
            install.load_bundle(bundle, '0' * 64)
        self.assertFalse((self.root / 'instance').exists())

class BundleAndExecutionTests(InstallFixture, unittest.TestCase):
    def setUp(self):
        super().setUp()
        release_path = Path(__file__).parents[3] / 'infra/release/publish.py'
        spec = importlib.util.spec_from_file_location('release_helper', release_path)
        publisher = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(publisher)
        self.sha = 'a' * 40
        self.digest = 'sha256:' + 'b' * 64
        self.image = 'ghcr.io/longkongjun/mediaflow@' + self.digest
        self.bundle = self.root / 'release'
        proof = publisher.proof_for(self.sha, self.digest, 42)
        record = dict(id=42, head_sha=self.sha, head_branch='main', event='push', status='completed',
                      conclusion='success', path='.github/workflows/ci.yml', repository={'full_name': publisher.REPO})
        publisher.bundle(self.bundle, 'v1.2.3', self.sha, proof, record)
        self.checksum = hashlib.sha256((self.bundle / 'checksums.txt').read_bytes()).hexdigest()
        self.config.update(uid=os.getuid() or 10001, gid=os.getgid() or 10001)

    def test_real_bundle_and_cli_preview_then_corruption_refusal(self):
        import subprocess
        config = self.root / 'input.json'
        config.write_text(json.dumps(self.config))
        script = Path(__file__).parents[1] / 'install.py'
        args = ['python3', '-B', str(script), 'plan', '--bundle', str(self.bundle),
                '--checksums-sha256', self.checksum, '--config', str(config)]
        result = subprocess.run(args, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout)['runtime_preflight'], 'not-run')
        self.assertFalse((self.root / 'instance').exists())
        (self.bundle / 'compose.yaml').write_text('tampered')
        result = subprocess.run(args, capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.root / 'instance').exists())

    def test_validated_bundle_rejects_symlinked_payload(self):
        payload = self.bundle / 'compose.yaml'
        copy = self.root / 'copied.yaml'
        payload.rename(copy)
        payload.symlink_to(copy)
        with self.assertRaises(ValueError):
            install.load_bundle(self.bundle, self.checksum)

    def fake_runtime(self, *args):
        if args[:3] == ('docker', 'image', 'inspect'):
            return json.dumps([dict(Architecture='amd64', Os='linux', RepoDigests=[self.image], Size=100,
                                   Config={'Labels': {'org.opencontainers.image.revision': self.sha,
                                                     'org.opencontainers.image.source': 'https://github.com/longkongjun/Indexed-Media',
                                                     'org.opencontainers.image.version': 'git-' + self.sha}})])
        if args[-3:] == ('config', '--format', 'json'):
            mounts = [
                ('config', '/config', False), ('deployment-roots.json', '/run/mediaflow/deployment-roots.json', True),
                ('secrets/bootstrap.secret', '/run/secrets/mediaflow-bootstrap', True)]
            volumes = [dict(type='bind', source=str(self.root / 'instance' / source), target=target, read_only=read_only)
                       for source, target, read_only in mounts]
            volumes += [dict(type='bind', source=self.config[key], target=target, read_only=read_only)
                        for key, target, read_only in [('incoming_dir', '/data/incoming', True), ('media_dir', '/data/media', False)]]
            return json.dumps({'services': {'mediaflow': dict(image=self.image, user=f'{self.config["uid"]}:{self.config["gid"]}',
                                                             read_only=True, cap_drop=['ALL'], volumes=volumes)}})
        return ''

    def test_failed_health_keeps_data_secret_and_diagnostics_and_cannot_reinstall(self):
        from unittest.mock import patch
        import subprocess
        calls = []
        def runtime(*args):
            calls.append(args)
            if 'up' in args:
                raise subprocess.CalledProcessError(7, ['docker', 'compose', 'up'], output='health timed out\n')
            return self.fake_runtime(*args)
        with patch.object(install.platform, 'system', return_value='Linux'), patch.object(install.platform, 'machine', return_value='x86_64'), patch.object(install, 'run', side_effect=runtime), patch.object(install.socket, 'socket'):
            with self.assertRaises(subprocess.CalledProcessError):
                install.perform(self.bundle, self.checksum, self.config, True)
        instance = self.root / 'instance'
        self.assertEqual(json.loads((instance / 'deployment.json').read_text())['state'], 'failed')
        secret = instance / 'secrets/bootstrap.secret'
        original = secret.read_bytes()
        self.assertEqual(secret.stat().st_mode & 0o777, 0o600)
        self.assertEqual(secret.stat().st_uid, self.config["uid"])
        self.assertIn('health timed out', (instance / 'install.log').read_text())
        self.assertNotIn(original.strip(), (instance / 'install.log').read_bytes())
        with self.assertRaises(ValueError):
            install.perform(self.bundle, self.checksum, self.config, True)
        self.assertEqual(secret.read_bytes(), original)
        self.assertFalse(any('down' in args for args in calls))

    def test_pull_failure_leaves_no_instance_and_a_protected_report(self):
        from unittest.mock import patch
        import subprocess
        def runtime(*args):
            if args[:2] == ("docker", "pull"):
                raise subprocess.CalledProcessError(7, list(args), output="registry unavailable")
            return self.fake_runtime(*args)
        with patch.object(install.platform, "system", return_value="Linux"), patch.object(install.platform, "machine", return_value="x86_64"), patch.object(install, "run", side_effect=runtime), patch.object(install.tempfile, "gettempdir", return_value=str(self.root)):
            with self.assertRaisesRegex(ValueError, "preflight report:") as error:
                install.perform(self.bundle, self.checksum, self.config, True)
        report = Path(str(error.exception).split("preflight report: ")[1])
        self.assertEqual(report.stat().st_mode & 0o777, 0o600)
        self.assertIn("registry unavailable", report.read_text())
        self.assertFalse((self.root / "instance").exists())

    def test_unexpected_compose_mount_is_refused_before_start(self):
        from unittest.mock import patch
        calls = []
        def runtime(*args):
            calls.append(args)
            result = self.fake_runtime(*args)
            if args[-3:] == ('config', '--format', 'json'):
                model = json.loads(result)
                model['services']['mediaflow'].setdefault('volumes', []).append(
                    dict(type='bind', source='/unexpected', target='/host', read_only=False))
                return json.dumps(model)
            return result
        with patch.object(install.platform, 'system', return_value='Linux'), patch.object(install.platform, 'machine', return_value='x86_64'), patch.object(install, 'run', side_effect=runtime), patch.object(install.socket, 'socket'):
            with self.assertRaisesRegex(ValueError, 'mount'):
                install.perform(self.bundle, self.checksum, self.config, True)
        self.assertFalse(any('up' in call for call in calls))

    def test_success_records_runtime_only_and_cannot_replace_instance(self):
        from unittest.mock import patch
        renamed = self.root / '媒体'
        (self.root / 'media').rename(renamed)
        self.config['media_dir'] = str(renamed)
        with patch.object(install.platform, 'system', return_value='Linux'), patch.object(install.platform, 'machine', return_value='x86_64'), patch.object(install, 'run', side_effect=self.fake_runtime), patch.object(install.socket, 'socket'):
            result = install.perform(self.bundle, self.checksum, self.config, True)
            self.assertEqual(result['state'], 'ready-for-initialization')
            self.assertIn('NAS checks still required', result['acceptance'])
            instance = self.root / 'instance'
            self.assertIn(str(renamed), (instance / '.env').read_text())
            self.assertEqual((instance / '.env').stat().st_mode & 0o777, 0o600)
            self.assertEqual((instance / 'config').stat().st_mode & 0o777, 0o700)
            with self.assertRaises(ValueError):
                install.perform(self.bundle, self.checksum, self.config, True)

    def test_lock_conflict_prevents_any_docker_or_instance_write(self):
        import fcntl
        from unittest.mock import patch
        instance = self.root / 'instance'
        project = 'mediaflow-' + hashlib.sha256(str(instance).encode()).hexdigest()[:12]
        with (self.root / f'.{project}.install.lock').open('w') as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            with patch.object(install.platform, 'system', return_value='Linux'), patch.object(install.platform, 'machine', return_value='x86_64'), patch.object(install, 'run') as runtime:
                with self.assertRaises(ValueError):
                    install.perform(self.bundle, self.checksum, self.config, True)
                runtime.assert_not_called()
        self.assertFalse(instance.exists())


if __name__ == '__main__':
    unittest.main()

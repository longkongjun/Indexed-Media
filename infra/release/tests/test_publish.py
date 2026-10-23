import copy
import importlib.util
import unittest
from pathlib import Path

SPEC = importlib.util.spec_from_file_location('publish', Path(__file__).parents[1] / 'publish.py')
publish = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(publish)
SHA = 'a' * 40
DIGEST = 'sha256:' + 'b' * 64


class ReleaseTests(unittest.TestCase):
    def setUp(self):
        self.run = {'id': 42, 'head_sha': SHA, 'head_branch': 'main', 'event': 'push',
                    'status': 'completed', 'conclusion': 'success', 'path': '.github/workflows/ci.yml',
                    'repository': {'full_name': 'longkongjun/Indexed-Media'}}

    def test_only_successful_trusted_main_can_supply_candidate(self):
        publish.check_run(self.run, SHA)
        for key, value in [('head_sha', 'c' * 40), ('head_branch', 'other'), ('event', 'pull_request'),
                           ('status', 'in_progress'), ('conclusion', 'skipped'), ('path', 'other.yml')]:
            bad = dict(self.run, **{key: value})
            with self.subTest(key=key), self.assertRaises(ValueError):
                publish.check_run(bad, SHA)
        bad = dict(self.run, repository={'full_name': 'someone/Indexed-Media'})
        with self.assertRaises(ValueError):
            publish.check_run(bad, SHA)

    def test_proof_is_bound_to_run_and_digest(self):
        proof = publish.proof_for(SHA, DIGEST, 42)
        publish.check_proof(proof, self.run, SHA)
        for key, value in [('run_id', 99), ('digest', 'latest'), ('platform', 'linux/arm64'),
                           ('source_sha', '0' * 40), ('image', 'attacker/image')]:
            bad = dict(proof, **{key: value})
            with self.subTest(key=key), self.assertRaises(ValueError):
                publish.check_proof(bad, self.run, SHA)

    def test_versions_and_identity_cannot_be_shell_inputs(self):
        self.assertEqual(publish.version('v1.2.3'), '1.2.3')
        for tag in ['v01.2.3', 'latest', 'v1.2', 'v1.2.3;id', 'v1.2.3-rc.1']:
            with self.subTest(tag=tag), self.assertRaises(ValueError):
                publish.version(tag)

    def test_existing_version_with_different_digest_is_never_replaced(self):
        publish.check_existing(DIGEST, DIGEST)
        with self.assertRaises(ValueError):
            publish.check_existing('sha256:' + 'c' * 64, DIGEST)

    def test_registry_auth_error_is_not_treated_as_absent_image(self):
        from unittest.mock import patch, Mock
        with patch.object(publish.subprocess, 'run', return_value=Mock(returncode=1, stderr='unauthorized: not found')):
            with self.assertRaises(ValueError):
                publish.remote_digest('ghcr.io/longkongjun/mediaflow:1.2.3')

    def test_both_public_packages_are_accepted(self):
        from unittest.mock import patch, Mock
        for name in ['mediaflow-dev', 'mediaflow']:
            with self.subTest(name=name), patch.object(publish.subprocess, 'run', return_value=Mock(returncode=0, stdout='{"visibility":"public"}')):
                publish.public_package(name)

    def test_nonpublic_or_unknown_visibility_is_rejected_even_before_creation(self):
        import json
        from unittest.mock import patch, Mock
        for visibility in ['private', 'internal', None]:
            for allow_missing in [False, True]:
                with self.subTest(visibility=visibility, allow_missing=allow_missing), patch.object(publish.subprocess, 'run', return_value=Mock(returncode=0, stdout=json.dumps({'visibility': visibility}))):
                    with self.assertRaisesRegex(ValueError, 'mediaflow must be public'):
                        publish.public_package('mediaflow', allow_missing=allow_missing)

    def test_visibility_api_errors_only_allow_initial_not_found(self):
        from unittest.mock import patch, Mock
        for code in [401, 403, 404, 500]:
            with self.subTest(code=code), patch.object(publish.subprocess, 'run', return_value=Mock(returncode=1, stderr=f'gh: API error (HTTP {code})')):
                with self.assertRaises(ValueError):
                    publish.public_package('mediaflow-dev')
                if code == 404:
                    publish.public_package('mediaflow-dev', allow_missing=True)
                else:
                    with self.assertRaises(ValueError):
                        publish.public_package('mediaflow-dev', allow_missing=True)

    def test_promotion_checks_exact_digest_and_never_builds(self):
        from unittest.mock import patch
        with patch.object(publish, 'remote_digest', side_effect=[None, 'sha256:' + 'c' * 64]), patch.object(publish, 'command') as command:
            with self.assertRaises(ValueError):
                publish.copy_image('source@' + DIGEST, 'target:1.2.3', DIGEST)
            args = command.call_args.args
            self.assertEqual(args[:4], ('docker', 'buildx', 'imagetools', 'create'))
            self.assertIn('--prefer-index=false', args)
            self.assertNotIn('build', args)

    def test_partial_draft_release_resumes_without_overwriting_assets(self):
        import tempfile
        import hashlib
        import json
        import subprocess
        from unittest.mock import patch, Mock
        with tempfile.TemporaryDirectory() as directory:
            bundle = Path(directory) / 'bundle'
            publish.bundle(bundle, 'v1.2.3', SHA, publish.proof_for(SHA, DIGEST, 42), self.run)
            assets = {}
            created = False
            draft = True
            fail_once = True
            edits = []
            def record():
                return dict(id=7, tag_name='v1.2.3', draft=draft, prerelease=False, assets=list(assets.values()))
            def command(*args):
                nonlocal created, draft, fail_once
                if args[:2] == ('gh', 'api'):
                    self.assertEqual(args[4], f'/repos/{publish.REPO}/releases?per_page=100')
                    self.assertIn('--paginate', args)
                    self.assertIn('--slurp', args)
                    return json.dumps([[dict(id=6, tag_name='v1.2.30')], [record()] if created else []])
                if args[:3] == ('gh', 'release', 'create'):
                    self.assertFalse(created, 'must resume the existing draft')
                    self.assertIn('--draft', args)
                    self.assertIn('--verify-tag', args)
                    created = True
                elif args[:3] == ('gh', 'release', 'upload'):
                    name = Path(args[4]).name
                    if name == 'roots.example.json' and fail_once:
                        fail_once = False
                        raise subprocess.CalledProcessError(1, args)
                    self.assertNotIn(name, assets)
                    assets[name] = dict(name=name, state='uploaded', digest='sha256:' + hashlib.sha256((bundle / name).read_bytes()).hexdigest())
                elif args[:3] == ('gh', 'release', 'edit'):
                    self.assertEqual(set(assets), set(publish.FILES) | {'checksums.txt'})
                    edits.append(args)
                    draft = False
                return ''
            def api_run(*args, **kwargs):
                return Mock(returncode=1 if draft else 0, stderr='gh: Not Found (HTTP 404)', stdout=json.dumps(record()))
            def api(path):
                if '/commits/' in path:
                    return {'sha': SHA}
                if path == f'/repos/{publish.REPO}/releases/7':
                    self.assertTrue(created)
                    return record()
                if '/releases/tags/' in path and draft:
                    raise subprocess.CalledProcessError(1, ['gh', 'api', path], stderr='gh: Not Found (HTTP 404)')
                self.fail(f'unexpected release API: {path}')
            with patch.object(publish.subprocess, 'run', side_effect=api_run), patch.object(publish, 'api', side_effect=api), patch.object(publish, 'command', side_effect=command):
                with self.assertRaises(subprocess.CalledProcessError) as failed:
                    publish.release('v1.2.3', bundle)
                self.assertEqual(failed.exception.cmd[:3], ('gh', 'release', 'upload'))
                self.assertEqual(len(assets), 3)
                self.assertTrue(draft)
                self.assertFalse(edits)
                publish.release('v1.2.3', bundle)
                self.assertFalse(draft)
                self.assertEqual(len(edits), 1)
                publish.release('v1.2.3', bundle)
                self.assertEqual(len(edits), 1)
                assets['compose.yaml']['digest'] = 'sha256:' + 'c' * 64
                with self.assertRaisesRegex(ValueError, 'existing Release asset differs'):
                    publish.release('v1.2.3', bundle)
                self.assertEqual(len(edits), 1)

    def test_stale_main_candidate_cannot_move_edge(self):
        import os, tempfile, json
        from unittest.mock import patch, Mock
        image = dict(Architecture="amd64", Os="linux", Config={"Labels": {
            "org.opencontainers.image.revision": SHA,
            "org.opencontainers.image.source": "https://github.com/" + publish.REPO,
            "org.opencontainers.image.version": "git-" + SHA}})
        env = dict(GITHUB_REPOSITORY=publish.REPO, GITHUB_EVENT_NAME="push", GITHUB_REF="refs/heads/main",
                   GITHUB_SHA=SHA, GITHUB_RUN_ID="42", GITHUB_RUN_ATTEMPT="1")
        with tempfile.TemporaryDirectory() as directory, patch.dict(os.environ, env), patch.object(publish.subprocess, 'run', return_value=Mock(returncode=0, stdout='{"visibility":"public"}')) as visibility, patch.object(publish, "copy_image"), patch.object(publish, "remote_digest", return_value=DIGEST), patch.object(publish, "api", return_value={"object": {"sha": "c" * 40}}), patch.object(publish, "command", return_value=json.dumps([image])) as calls:
            proof = Path(directory) / "candidate.json"
            publish.candidate(proof)
            self.assertEqual(json.loads(proof.read_text())["source_sha"], SHA)
            self.assertEqual(visibility.call_count, 2)
            self.assertFalse(any(publish.DEV + ":edge" in call.args for call in calls.call_args_list))
            image["Architecture"] = "arm64"
            calls.return_value = json.dumps([image])
            calls.reset_mock()
            with self.assertRaises(ValueError):
                publish.candidate(proof)
            self.assertFalse(any("push" in call.args or "tag" in call.args for call in calls.call_args_list))

    def test_release_refuses_source_or_release_tag_mismatch_before_writing(self):
        import hashlib, json, tempfile
        from unittest.mock import patch
        with tempfile.TemporaryDirectory() as directory:
            bundle = Path(directory) / 'bundle'
            publish.bundle(bundle, 'v1.2.3', SHA, publish.proof_for(SHA, DIGEST, 42), self.run)
            assets = [dict(name=name, state='uploaded', digest='sha256:' + hashlib.sha256((bundle / name).read_bytes()).hexdigest())
                      for name in (*publish.FILES, 'checksums.txt')]
            for source, release_tag, message in [('c' * 40, 'v1.2.3', 'tag and bundle source differ'),
                                                  (SHA, 'v9.9.9', 'Release tag differs')]:
                record = dict(id=7, tag_name=release_tag, draft=False, prerelease=False, assets=assets)
                with self.subTest(source=source, release_tag=release_tag), patch.object(publish, 'find_release', return_value=record), patch.object(publish, 'api', side_effect=lambda path: {'sha': source} if '/commits/' in path else record), patch.object(publish, 'command') as command:
                    with self.assertRaisesRegex(ValueError, message):
                        publish.release('v1.2.3', bundle)
                    command.assert_not_called()

    def test_candidate_from_other_event_cannot_even_inspect_artifacts(self):
        from unittest.mock import patch
        bad = dict(self.run, event='pull_request')
        with patch.object(publish, 'api', return_value={'workflow_runs': [bad]}) as api:
            with self.assertRaises(ValueError):
                publish.trusted_candidate(SHA)
            self.assertEqual(api.call_count, 1)

    def test_release_lookup_rejects_api_errors_and_ambiguous_tag(self):
        import json
        import subprocess
        from unittest.mock import patch
        with patch.object(publish, 'command', side_effect=subprocess.CalledProcessError(1, ['gh'], stderr='HTTP 403')):
            with self.assertRaises(subprocess.CalledProcessError):
                publish.find_release('v1.2.3')
        with patch.object(publish, 'command', return_value=json.dumps([[{'tag_name': 'v1.2.3', 'id': 7}], [{'tag_name': 'v1.2.3', 'id': 8}]])):
            with self.assertRaisesRegex(ValueError, 'multiple Releases'):
                publish.find_release('v1.2.3')


if __name__ == '__main__':
    unittest.main()

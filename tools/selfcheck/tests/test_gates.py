"""Exercise real entry points with isolated external tools; no native build claims."""

import os
import re
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


REPO = Path(__file__).resolve().parents[3]
JUST = shutil.which("just")


def workflow_policy_errors(text):
    # Repository workflows use one-line actions and explicit permission mappings.
    errors = []
    lines = text.splitlines()
    permission_blocks = 0
    for index, line in enumerate(lines):
        if line.lstrip().startswith("#"):
            continue
        if re.search(r"\buses\s*:", line) and not re.fullmatch(
            r"\s*-?\s*uses:\s+[\w.-]+/[\w./-]+@[0-9a-f]{40}\s*(#.*)?", line
        ):
            errors.append("action must use a full SHA")
        match = re.fullmatch(r"(\s*)permissions:(.*)", line)
        if match:
            permission_blocks += 1
            indent = len(match[1])
            block = []
            for child in lines[index + 1:]:
                if not child.strip() or child.lstrip().startswith("#"):
                    continue
                if len(child) - len(child.lstrip()) <= indent:
                    break
                block.append(child.strip())
            if match[2].strip() or block != ["contents: read"]:
                errors.append("token permissions must be explicit contents: read")
    if not permission_blocks or not any(line == "permissions:" for line in lines):
        errors.append("top-level permissions required")
    if any(re.match(r"\s*pull_request_target\s*:", line) for line in lines):
        errors.append("untrusted pull_request_target is not allowed")
    return errors


class WorkflowPolicyTests(unittest.TestCase):
    def test_actual_workflows_pin_actions_and_keep_token_readonly(self):
        for path in (REPO / ".github/workflows").glob("*.yml"):
            with self.subTest(path=path.name):
                self.assertEqual(workflow_policy_errors(path.read_text()), [])

    def test_floating_action_is_rejected(self):
        self.assertIn("action must use a full SHA", workflow_policy_errors(
            "permissions:\n  contents: read\nsteps:\n  - uses: actions/checkout@v4\n"))

    def test_job_permission_override_is_rejected(self):
        self.assertIn("token permissions must be explicit contents: read", workflow_policy_errors(
            "permissions:\n  contents: read\njobs:\n  build:\n    permissions:\n      contents: write\n"))


class GateTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="mediaflow-gate-test-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.bin = self.root / "bin"
        self.bin.mkdir()
        self.reports = self.root / "reports"
        self.reports.mkdir()
        (self.root / "apps/ios").mkdir(parents=True)
        (self.root / "tools/selfcheck").mkdir(parents=True)
        shutil.copy(REPO / "justfile", self.root / "justfile")
        for script in (REPO / "tools/selfcheck").glob("*.sh"):
            shutil.copy(script, self.root / "tools/selfcheck")
        self.env = dict(os.environ, PATH=f"{self.bin}:/usr/bin:/bin", TMPDIR=str(self.reports))
        self.env.pop("DEVELOPER_DIR", None)
        for tool in ["cargo", "cargo-deny", "cargo-machete", "pnpm", "node", "docker", "xcrun"]:
            self.tool(tool, "exit 0")
        shutil.copy(REPO / "tools/selfcheck/tool-versions.env", self.root / "tools/selfcheck")
        self.tool("swiftlint", 'if [ "$1" = version ]; then echo 0.65.1; fi; exit "${SWIFTLINT_STATUS:-0}"')
        self.tool("xcodegen", 'echo generator-output; exit "${GENERATOR_STATUS:-0}"')
        self.tool("xcodebuild", 'echo first-build-line; seq 1 80; exit "${BUILD_STATUS:-0}"')
        self.tool("gradlew", "exit 0", self.root / "gradlew")

    def tool(self, name, body, path=None):
        path = path or self.bin / name
        path.write_text("#!/bin/sh\n" + body + "\n")
        path.chmod(0o755)

    def run_gate(self, *args, **env):
        return subprocess.run(args, cwd=self.root, env=dict(self.env, **env),
                              text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)

    def ios(self, recipe="client-check-ios", **env):
        # Keep recursive just calls real while all external build tools are isolated.
        self.tool("just", f'exec "{JUST}" "$@"')
        return self.run_gate(JUST, recipe, **env)

    def selfcheck(self, offline=True, **env):
        self.tool("just", '''echo "$*" >> "$TMPDIR/calls"
case "$1" in
  check|check-source) if [ "${SOURCE_FAIL:-0}" = 1 ]; then
    echo 'failed to fetch advisory database'; echo 'independent source error'; exit 7
  fi;;
esac
if [ "$1" = "${FAIL_RECIPE:-}" ]; then echo independent-error; exit 9; fi
exit 0''')
        args = ["/bin/sh", "tools/selfcheck/selfcheck.sh"]
        if offline:
            args.append("--offline")
        return self.run_gate(*args, **env)

    def test_ios_build_failure_retains_full_log_and_status(self):
        result = self.ios(BUILD_STATUS="65")
        self.assertNotEqual(result.returncode, 0, result.stdout)
        logs = list(self.reports.rglob("xcodebuild.log"))
        self.assertEqual(len(logs), 1, result.stdout)
        self.assertIn("first-build-line", logs[0].read_text())
        self.assertIn("xcodebuild\t65", (logs[0].parent / "status.tsv").read_text())

    def test_ios_generator_failure_does_not_launch_build(self):
        result = self.ios(GENERATOR_STATUS="17")
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertFalse(list(self.reports.rglob("xcodebuild.log")))
        logs = list(self.reports.rglob("xcodegen.log"))
        self.assertEqual(len(logs), 1, result.stdout)
        self.assertIn("xcodegen\t17", (logs[0].parent / "status.tsv").read_text())

    def test_ios_success_uses_unique_report_directories(self):
        for _ in range(2):
            result = self.ios()
            self.assertEqual(result.returncode, 0, result.stdout)
        self.assertEqual(len(list(self.reports.rglob("xcodebuild.log"))), 2)

    def test_offline_advisory_is_pending_and_other_checks_run(self):
        result = self.selfcheck()
        self.assertEqual(result.returncode, 2, result.stdout)
        calls = (self.reports / "calls").read_text()
        self.assertIn("check-rust-deps-static", calls)
        self.assertIn("check-nas", calls)
        self.assertNotIn("全部通过", result.stdout)

    def test_offline_advisory_text_cannot_hide_source_failure(self):
        result = self.selfcheck(SOURCE_FAIL="1")
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("check-nas", (self.reports / "calls").read_text())
        self.assertNotIn("全部通过", result.stdout)

    def test_failed_middle_check_does_not_skip_last_check(self):
        result = self.selfcheck(FAIL_RECIPE="check-compose-static")
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("check-nas", (self.reports / "calls").read_text())

    def test_ios_failure_reaches_legacy_and_client_aggregates(self):
        for recipe in ["check-ios", "check-mobile", "client-check"]:
            with self.subTest(recipe=recipe):
                result = self.ios(recipe, BUILD_STATUS="65")
                self.assertNotEqual(result.returncode, 0, result.stdout)

    def test_online_advisory_failure_does_not_skip_static_checks(self):
        result = self.selfcheck(offline=False, FAIL_RECIPE="check-rust-advisories")
        self.assertEqual(result.returncode, 1, result.stdout)
        self.assertIn("check-nas", (self.reports / "calls").read_text())

    def test_selfcheck_reports_missing_required_tool(self):
        (self.bin / "cargo-deny").unlink()
        result = self.selfcheck()
        self.assertEqual(result.returncode, 2, result.stdout)
        self.assertIn("cargo-deny", result.stdout)
        self.assertFalse((self.reports / "calls").exists())
        self.assertTrue(any("environment:cargo-deny\tPENDING" in p.read_text()
                            for p in self.reports.rglob("status.tsv")))

    def test_required_groups_propagate_first_middle_and_last_failure(self):
        for failed in ["check", "client-check", "check-containers", "check-compose", "check-nas"]:
            with self.subTest(failed=failed):
                self.tool("just", 'if [ "$1" = "$FAIL_RECIPE" ]; then exit 7; fi; exit 0')
                result = self.run_gate(JUST, "check-all", FAIL_RECIPE=failed)
                self.assertNotEqual(result.returncode, 0, result.stdout)

    def test_ios_failure_reaches_complete_entry(self):
        self.tool("just", f'''case "$1" in
check|check-containers|check-compose|check-nas) exit 0;;
*) exec "{JUST}" "$@";;
esac''')
        result = self.run_gate(JUST, "check-all", BUILD_STATUS="65")
        self.assertNotEqual(result.returncode, 0, result.stdout)
        logs = list(self.reports.rglob("status.tsv"))
        self.assertTrue(any("xcodebuild\t65" in p.read_text() for p in logs), result.stdout)

    def test_swiftlint_failure_prevents_native_build(self):
        result = self.ios(SWIFTLINT_STATUS="3")
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertFalse(list(self.reports.rglob("xcodegen.log")))

    def test_installer_rejects_wrong_hash_before_unpacking(self):
        self.tool("curl", 'while [ "$1" != "-o" ]; do shift; done; printf bad-archive > "$2"')
        self.tool("unzip", 'touch "$TMPDIR/unpacked"')
        result = self.run_gate("/bin/bash", "tools/selfcheck/install-swiftlint.sh", str(self.root / "download"))
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertFalse((self.reports / "unpacked").exists())


if __name__ == "__main__":
    unittest.main()

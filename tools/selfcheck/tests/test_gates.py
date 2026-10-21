"""Exercise real entry points with isolated external tools; no native build claims."""

import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


REPO = Path(__file__).resolve().parents[3]
JUST = shutil.which("just")


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
        for tool in ["cargo", "cargo-deny", "cargo-machete", "pnpm", "node", "docker", "swiftlint"]:
            self.tool(tool, "exit 0")
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


if __name__ == "__main__":
    unittest.main()

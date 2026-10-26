import importlib.util
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("timing", Path(__file__).with_name("record-recovery-timing.py"))
timing = importlib.util.module_from_spec(spec)
spec.loader.exec_module(timing)


class TimingEvidenceTests(unittest.TestCase):
    def reference(self):
        return {"name": "local-reference", "hardware": "test-hardware", "os": "test-os", "filesystem": "apfs", "cpu_count": 10, "memory_bytes": 34359738368, "temp_path_depth": 7, "max_load_per_cpu": 1}

    def test_reference_mismatch_and_host_pressure_are_not_a_pass(self):
        reference = self.reference()
        timing.validate_reference(reference)
        actual = {**reference, "load": [11, 0, 0]}
        self.assertFalse(timing.qualified(reference, actual))
        actual.update(load=[1, 0, 0], filesystem="ext4")
        self.assertFalse(timing.qualified(reference, actual))

    def test_missing_or_fake_reference_is_rejected(self):
        for update in ({"name": "todo"}, {"max_load_per_cpu": 4}, {"cpu_count": True}):
            with self.assertRaises(ValueError):
                timing.validate_reference({**self.reference(), **update})

    def test_inexact_or_missing_samples_are_rejected(self):
        sample = {"scenario": "restart-scan", "elapsed_ns": 1, "files": 15000, "observed": 15000, "unique": 15000, "recoveries": 1, "same_batch": True}
        log = "MEDIAFLOW_TIMING " + json.dumps(sample)
        self.assertEqual(timing.timing_sample(log, "restart-scan"), sample)
        for invalid in ("", log+"\n"+log, "MEDIAFLOW_TIMING "+json.dumps({**sample, "unique": 14999})):
            with self.assertRaises(ValueError):
                timing.timing_sample(invalid, "restart-scan")

        native = {"scenario":"native-scan","elapsed_ns":1,"base_files":1201,"raw_created":False,"observed":1201,"directories":131,"skipped":1,"errors":1}
        with self.assertRaises(ValueError):
            timing.timing_sample("MEDIAFLOW_TIMING "+json.dumps(native),"native-scan")

    def test_failed_evidence_is_atomic_and_cannot_overwrite(self):
        with tempfile.TemporaryDirectory() as root:
            output = Path(root)/"result.json"
            timing.atomic_write(output, {"status": "failed"})
            with self.assertRaises(FileExistsError):
                timing.atomic_write(output, {"status": "passed"})
            self.assertEqual(json.loads(output.read_text())["status"], "failed")
            self.assertEqual(len(list(Path(root).iterdir())), 1)
            with self.assertRaises(ValueError):
                timing.atomic_write(Path(root)/"invalid.json", {"status":"passed","invalid":float("nan")})
            self.assertEqual(len(list(Path(root).iterdir())), 1)


    def test_original_total_budgets_still_fail_after_correctness_succeeds(self):
        reference = self.reference()
        actual = {**reference, "load": [1, 0, 0]}
        samples = [
            {"scenario": "native-scan", "elapsed_ns": 5_000_000_001, "base_files": 1201, "raw_created": False, "observed": 1201, "directories": 131, "skipped": 1, "errors": 0},
            {"scenario": "restart-scan", "elapsed_ns": 30_000_000_001, "files": 15000, "observed": 15000, "unique": 15000, "recoveries": 1, "same_batch": True},
        ]
        self.assertEqual(len(timing.CASES), len(samples))
        for case, sample in zip(timing.CASES, samples):
            with self.subTest(case=case), tempfile.TemporaryDirectory() as root:
                ref = Path(root)/"reference.json"
                ref.write_text(json.dumps(reference))
                output = Path(root)/"result.json"
                with patch.object(timing, "environment", return_value=actual), patch.object(timing, "source_digest", return_value="source"), patch.object(timing, "CASES", [case]), patch.object(timing.subprocess, "check_output", side_effect=["sha", b"", "rustc"]), patch.object(timing.subprocess, "run", return_value=SimpleNamespace(returncode=0, stdout="MEDIAFLOW_TIMING "+json.dumps(sample))):
                    self.assertEqual(timing.main(["--reference",str(ref),"--output",str(output)]),1)
                result = json.loads(output.read_text())
                self.assertEqual(result["status"],"failed")
                self.assertIn("budget exceeded",result["runs"][0]["error"])

    def test_success_requires_correctness_and_unchanged_source(self):
        reference = self.reference()
        actual = {**reference, "load": [1, 0, 0]}
        sample = {"scenario": "restart-scan", "elapsed_ns": 30_000_000_000, "files": 15000, "observed": 15000, "unique": 15000, "recoveries": 1, "same_batch": True}
        for code, hashes, expected in [(0,["source","source"],0),(1,["source","source"],1),(0,["before","after"],1)]:
            with self.subTest(code=code,hashes=hashes), tempfile.TemporaryDirectory() as root:
                ref = Path(root)/"reference.json"
                ref.write_text(json.dumps(reference))
                output = Path(root)/"result.json"
                with patch.object(timing, "environment", return_value=actual), patch.object(timing, "source_digest", side_effect=hashes), patch.object(timing, "CASES", timing.CASES[1:]), patch.object(timing.subprocess, "check_output", side_effect=["sha", b"", "rustc"]), patch.object(timing.subprocess, "run", return_value=SimpleNamespace(returncode=code,stdout="MEDIAFLOW_TIMING "+json.dumps(sample))):
                    self.assertEqual(timing.main(["--reference",str(ref),"--output",str(output)]),expected)
                self.assertEqual(json.loads(output.read_text())["status"],"passed" if expected==0 else "failed")

    def test_unqualified_environment_records_pending_without_running_cases(self):
        reference = self.reference()
        with tempfile.TemporaryDirectory() as root:
            ref = Path(root)/"reference.json"
            ref.write_text(json.dumps(reference))
            output = Path(root)/"result.json"
            with patch.object(timing, "environment", return_value={**reference,"load":[20,0,0]}), patch.object(timing, "source_digest", return_value="source"), patch.object(timing.subprocess, "check_output", side_effect=["sha", b"", "rustc"]), patch.object(timing.subprocess, "run") as run:
                self.assertEqual(timing.main(["--reference",str(ref),"--output",str(output)]),2)
                run.assert_not_called()
            self.assertEqual(json.loads(output.read_text())["status"],"environment-unavailable")


if __name__ == "__main__":
    unittest.main()

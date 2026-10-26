#!/usr/bin/env python3
"""Run native correctness cases and separately enforce the original 5s/30s budgets."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import plistlib
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[3]
CASES = (
    ("native-scan", "scan_worker", "bounded_scanner_streams_batches_preserves_raw_names_and_skips_symlinks", 5_000_000_000),
    ("restart-scan", "recovery_capacity", "independent_core_kill_and_restart_recovers_same_task_and_batch_once", 30_000_000_000),
)


def environment():
    system = platform.system()
    temporary = tempfile.gettempdir()
    if system == "Darwin":
        hardware = subprocess.check_output(["sysctl", "-n", "hw.model"], text=True).strip()
        memory = int(subprocess.check_output(["sysctl", "-n", "hw.memsize"], text=True))
        mount = subprocess.check_output(["df", "-P", temporary], text=True).splitlines()[-1].split()[-1]
        filesystem = plistlib.loads(subprocess.check_output(["diskutil", "info", "-plist", mount]))["FilesystemType"]
    else:
        hardware = platform.machine()
        memory = os.sysconf("SC_PHYS_PAGES") * os.sysconf("SC_PAGE_SIZE")
        filesystem = subprocess.check_output(["stat", "-f", "-c", "%T", temporary], text=True).strip()
    return {"hardware": hardware, "os": f"{system} {platform.mac_ver()[0] or platform.release()}",
            "filesystem": filesystem.lower(), "cpu_count": os.cpu_count(), "memory_bytes": memory,
            "temp_path_depth": len(Path(temporary).resolve().parts), "load": list(os.getloadavg())}


def validate_reference(reference):
    if not isinstance(reference, dict):
        raise ValueError("reference must be a JSON object")
    for key in ("name", "hardware", "os", "filesystem"):
        value = reference.get(key)
        if not isinstance(value, str) or not value.strip() or value.lower() in {"unknown", "todo", "example", "placeholder"}:
            raise ValueError(f"reference requires a real {key}")
    for key in ("cpu_count", "memory_bytes", "temp_path_depth"):
        if type(reference.get(key)) is not int or reference[key] < 1:
            raise ValueError(f"reference requires {key}")
    limit = reference.get("max_load_per_cpu")
    if type(limit) not in (int, float) or not 0 < limit <= 1:
        raise ValueError("reference max_load_per_cpu must be in (0, 1]")


def qualified(reference, actual):
    return (all(reference[key] == actual[key] for key in ("hardware", "os", "filesystem", "cpu_count", "memory_bytes", "temp_path_depth"))
            and actual["load"][0] / actual["cpu_count"] <= reference["max_load_per_cpu"])


def timing_sample(log, scenario):
    records = [json.loads(line.split("MEDIAFLOW_TIMING ", 1)[1]) for line in log.splitlines() if "MEDIAFLOW_TIMING " in line]
    if len(records) != 1 or not isinstance(records[0], dict) or records[0].get("scenario") != scenario:
        raise ValueError("exactly one matching correctness-complete timing sample is required")
    sample = records[0]
    if type(sample.get("elapsed_ns")) is not int or sample["elapsed_ns"] <= 0:
        raise ValueError("elapsed_ns must be a positive integer")
    numeric = ("base_files", "observed", "directories", "skipped", "errors") if scenario == "native-scan" else ("files", "observed", "unique", "recoveries")
    if any(type(sample.get(key)) is not int for key in numeric):
        raise ValueError("dataset and result counts must be integers")
    if scenario == "native-scan":
        if (sample.get("base_files") != 1201 or type(sample.get("raw_created")) is not bool
                or sample.get("observed") != 1201 + sample["raw_created"]
                or sample.get("directories") != 131 or sample.get("skipped") != 1 or sample.get("errors") != 0):
            raise ValueError("native dataset or result differs from the original case")
    elif (sample.get("files") != 15000 or sample.get("observed") != 15000
          or sample.get("unique") != 15000 or sample.get("recoveries") != 1 or sample.get("same_batch") is not True):
        raise ValueError("restart dataset or result differs from the original case")
    return sample


def atomic_write(path, evidence):
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(mode="w", dir=path.parent, delete=False) as stream:
            temporary = Path(stream.name)
            json.dump(evidence, stream, ensure_ascii=False, indent=2, allow_nan=False)
            stream.write("\n")
            stream.flush()
            os.fsync(stream.fileno())
        os.link(temporary, path)  # Publish the complete record without overwriting an existing result.
    finally:
        if temporary is not None:
            temporary.unlink()


def source_digest():
    paths = subprocess.check_output(["git", "ls-files", "--cached", "--others", "--exclude-standard", "-z", "apps/core"], cwd=ROOT).decode().split("\0")
    digest = hashlib.sha256()
    for name in sorted(set(paths) - {""}):
        path = ROOT / name
        value = os.readlink(path).encode() if path.is_symlink() else path.read_bytes() if path.exists() else b"deleted"
        digest.update(name.encode() + b"\0" + hashlib.sha256(value).digest())
    return digest.hexdigest()


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--describe-environment", action="store_true")
    parser.add_argument("--reference", type=Path)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args(argv)
    if args.describe_environment:
        print(json.dumps(environment(), ensure_ascii=False, indent=2))
        return 0
    if not args.reference or not args.output or not args.output.is_absolute() or not args.output.parent.is_dir() or args.output.exists():
        parser.error("use --reference FILE --output NEW_ABSOLUTE_FILE with an existing parent")
    reference = json.loads(args.reference.read_text())
    validate_reference(reference)
    evidence = {"reference": reference, "profile": "test-opt0", "head": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
                "source_dirty": bool(subprocess.check_output(["git", "status", "--porcelain"], cwd=ROOT)),
                "toolchain": subprocess.check_output(["rustc", "--version"], cwd=ROOT, text=True).strip(), "runs": [], "status": "environment-unavailable"}
    evidence["source_digest"] = source_digest()
    initial = environment()
    evidence["initial_environment"] = initial
    if not qualified(reference, initial):
        atomic_write(args.output, evidence)
        return 2
    if any(os.environ.get(key) for key in ("RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS")):
        raise ValueError("custom Rust flags are not part of the test-opt0 reference profile")
    env = os.environ.copy()
    env.update(CARGO_PROFILE_TEST_OPT_LEVEL="0", CARGO_PROFILE_DEV_OPT_LEVEL="0", CARGO_PROFILE_TEST_DEBUG="0", CARGO_PROFILE_DEV_DEBUG="0", CARGO_INCREMENTAL="0")
    evidence["profile_environment"] = {key: value for key, value in env.items() if key.startswith("CARGO_PROFILE_") or key == "CARGO_INCREMENTAL"}
    evidence["status"] = "passed"
    for scenario, target, case, budget in CASES:
        command = ["cargo", "test", "--manifest-path", "apps/core/Cargo.toml", "--locked", "--test", target, case, "--", "--exact", "--nocapture"]
        run = subprocess.run(command, cwd=ROOT, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
        log_path = args.output.with_name(args.output.name + f".{scenario}.log")
        with log_path.open("x") as stream:
            stream.write(run.stdout)
        result = {"command": command, "exit_code": run.returncode, "log": str(log_path), "budget_ns": budget}
        evidence["runs"].append(result)
        try:
            if run.returncode:
                raise ValueError("correctness case failed; no performance pass")
            result["sample"] = timing_sample(run.stdout, scenario)
            if result["sample"]["elapsed_ns"] > budget:
                raise ValueError("original total elapsed budget exceeded")
        except ValueError as error:
            result["error"] = str(error)
            evidence["status"] = "failed"
        result["environment_after"] = environment()
        if not qualified(reference, result["environment_after"]):
            if evidence["status"] != "failed":
                evidence["status"] = "environment-unavailable"
            break
    evidence["source_digest_after"] = source_digest()
    if evidence["source_digest_after"] != evidence["source_digest"]:
        evidence["status"] = "failed"
        evidence["error"] = "source changed during measurement"
    atomic_write(args.output, evidence)
    return {"passed": 0, "failed": 1, "environment-unavailable": 2}[evidence["status"]]


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        print(f"recovery timing: {error}", file=sys.stderr)
        sys.exit(2)

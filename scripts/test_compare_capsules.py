import json
import pathlib
import subprocess
import sys
import tempfile
import unittest


SCRIPT = pathlib.Path(__file__).with_name("compare-capsules.py")


def metric(name, value, kind, method, unit):
    return {
        "name": name,
        "value": value,
        "kind": kind,
        "method": method,
        "unit": unit,
    }


def capsule(fingerprint):
    metrics = [
        metric("instructions", 1000, "simulated", "cachegrind", "count"),
        metric("allocations", 4, "measured", "valgrind_memcheck", "count"),
        metric("binary_size", 4096, "measured", "stat", "bytes"),
        metric("stdout_bytes", 32, "measured", "runner", "bytes"),
        metric("instructions_hw", 500, "measured", "perf", "count"),
        metric("peak_memory", 65536, "indicative", "getrusage", "bytes"),
        metric("user_cpu_time", 250000, "measured", "getrusage", "nanoseconds"),
        metric("wall_clock", 400000, "indicative", "wall_clock", "nanoseconds"),
    ]
    return {
        "schema_version": 14,
        "problem": {"id": "sorted-pair-sum", "statement_sha256": "b" * 64},
        "source": {"source_sha256": "c" * 64},
        "toolchain": {
            "name": "gcc",
            "version": "14.2.0",
            "environment_fingerprint": "sha256:" + "f" * 64,
        },
        "build": {
            "binary_sha256": "sha256:" + "9" * 64,
            "build_repeat": 3,
            "argv": ["g++", "-O2", "main.cpp"],
            "opt_level": "-O2",
            "stdin_sha256": "d" * 64,
        },
        "reference_build": {
            "binary_sha256": "sha256:" + "8" * 64,
            "argv": ["g++", "-O2", "-std=c++20", "/src0/reference.cpp"],
        },
        "harness": {"version": "0.13.0", "ruleset": "phase1"},
        "input": {
            "generator": "generator-v1",
            "seed": 42,
            "seed_set": "public",
            "size": "good",
        },
        "verdict": "accepted",
        "host": {"fingerprint": fingerprint},
        "metrics": metrics,
        "repeatability": {
            "runs": 20,
            "per_metric": {
                item["name"]: {
                    "min": item["value"],
                    "max": item["value"],
                    "median": item["value"],
                    "spread_pct": 0.0,
                }
                for item in metrics
                if item["kind"] == "simulated"
                or (
                    item["kind"] == "measured"
                    and item["method"]
                    in ("manual", "valgrind_memcheck", "stat", "runner")
                )
            },
        },
    }


def toolchain_lock():
    return {
        "apt_snapshot": "2026-10-01T00:00:00Z",
        "harness_sha256": "a" * 64,
        "toolchains": [
            {
                "name": "gcc",
                "version": "14.2.0",
                "libc": "glibc-2.41",
                "compiler_sha256": "e" * 64,
                "environment_fingerprint": "sha256:" + "f" * 64,
            }
        ],
    }


class CompareCapsulesTests(unittest.TestCase):
    def run_comparison(self, capsules, max_spread=None):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            for index, label in enumerate(("ubuntu-22.04", "ubuntu-24.04", "ubuntu-26.04")):
                (root / f"capsule-{label}.json").write_text(json.dumps(capsules[index]))
                (root / f"toolchains-{label}.json").write_text(json.dumps(toolchain_lock()))
            command = [sys.executable, str(SCRIPT), str(root)]
            if max_spread is not None:
                command.extend(["--max-within-host-spread-pct", str(max_spread)])
            return subprocess.run(command, capture_output=True, text=True, check=False)

    def setUp(self):
        self.capsules = [
            capsule("host-22"),
            capsule("host-24"),
            capsule("host-26"),
        ]

    def test_exact_deterministic_metrics_pass_and_machine_metrics_are_excluded(self):
        for index, value in enumerate((900, 1100, 1300)):
            self.capsules[index]["metrics"][4]["value"] = value
            self.capsules[index]["metrics"][5]["value"] = value * 100
            self.capsules[index]["metrics"][6]["value"] = value * 200
            self.capsules[index]["metrics"][7]["value"] = value * 300
        result = self.run_comparison(self.capsules)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("all deterministic metrics stayed within 0.010000%", result.stdout)

    def test_independent_runners_may_share_a_host_fingerprint(self):
        for capsule in self.capsules:
            capsule["host"]["fingerprint"] = "same-host-class"
        result = self.run_comparison(self.capsules)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("host fingerprints recorded for 3 runner environments", result.stdout)

    def test_within_host_spread_above_gate_fails(self):
        per_metric = self.capsules[1]["repeatability"]["per_metric"]["instructions"]
        per_metric["max"] = 1000.2
        per_metric["spread_pct"] = 0.02
        result = self.run_comparison(self.capsules)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("within-host spread", result.stderr)
        self.assertIn("exceeds the 0.010000% gate", result.stderr)

    def test_cross_host_deterministic_difference_fails(self):
        self.capsules[2]["metrics"][0]["value"] = 1001
        result = self.run_comparison(self.capsules)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("instructions differs across machines", result.stderr)

    def test_environment_fingerprint_mismatch_fails(self):
        self.capsules[1]["toolchain"]["environment_fingerprint"] = "sha256:" + "1" * 64
        result = self.run_comparison(self.capsules)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("capsule identity differs at toolchain.environment_fingerprint", result.stderr)

    def test_binary_digest_mismatch_fails(self):
        self.capsules[2]["build"]["binary_sha256"] = "sha256:" + "8" * 64
        result = self.run_comparison(self.capsules)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("capsule identity differs at build.binary_sha256", result.stderr)

    def test_reference_binary_digest_mismatch_fails(self):
        self.capsules[2]["reference_build"]["binary_sha256"] = "sha256:" + "7" * 64
        result = self.run_comparison(self.capsules)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("capsule identity differs at reference_build.binary_sha256", result.stderr)

    def test_build_repeat_mismatch_fails(self):
        self.capsules[2]["build"]["build_repeat"] = 4
        result = self.run_comparison(self.capsules)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("capsule identity differs at build.build_repeat", result.stderr)

    def test_explicit_spread_gate_is_respected(self):
        per_metric = self.capsules[0]["repeatability"]["per_metric"]["allocations"]
        per_metric["min"] = 3.99
        per_metric["max"] = 4.0
        per_metric["spread_pct"] = 0.25
        result = self.run_comparison(self.capsules, max_spread=0.3)
        self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == "__main__":
    unittest.main()

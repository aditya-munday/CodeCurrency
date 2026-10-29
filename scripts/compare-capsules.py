import json
import math
import pathlib
import sys
import argparse


parser = argparse.ArgumentParser()
parser.add_argument("directory", nargs="?", default=".")
parser.add_argument("--toolchain")
parser.add_argument("--profile")
parser.add_argument("--max-within-host-spread-pct", type=float, default=0.01)
arguments = parser.parse_args()
root = pathlib.Path(arguments.directory)
max_spread_pct = arguments.max_within_host_spread_pct
if not math.isfinite(max_spread_pct) or max_spread_pct < 0:
    raise SystemExit("maximum within-host spread must be a finite non-negative percentage")
selector = arguments.profile or arguments.toolchain
pattern = f"capsule-*-{selector}.json" if selector else "capsule-*.json"
capsule_paths = sorted(root.glob(pattern))
if len(capsule_paths) != 3:
    raise SystemExit(f"expected exactly 3 capsules in {root}, found {len(capsule_paths)}")

capsules = [json.loads(path.read_text()) for path in capsule_paths]
runner_names = [
    path.stem.removeprefix("capsule-").removesuffix(f"-{selector}")
    if selector
    else path.stem.removeprefix("capsule-")
    for path in capsule_paths
]
locks = [json.loads(root.joinpath(f"toolchains-{runner}.json").read_text()) for runner in runner_names]

for field in ("apt_snapshot", "harness_sha256"):
    values = {lock.get(field) for lock in locks}
    if len(values) != 1:
        raise SystemExit(f"toolchain lock mismatch for {field}: {sorted(map(str, values))}")

expected_toolchain = arguments.toolchain or capsules[0].get("toolchain", {}).get("name")
toolchain_entries = []
for lock, capsule, path in zip(locks, capsules, capsule_paths):
    if capsule.get("toolchain", {}).get("name") != expected_toolchain:
        raise SystemExit(f"{path.name}: capsule toolchain does not match {expected_toolchain}")
    entry = next((item for item in lock["toolchains"] if item["name"] == expected_toolchain), None)
    if entry is None:
        raise SystemExit(f"{path.name}: toolchain lock is missing {expected_toolchain}")
    if capsule.get("toolchain", {}).get("version") != entry.get("version"):
        raise SystemExit(f"{path.name}: capsule toolchain version does not match its lock")
    if capsule.get("toolchain", {}).get("environment_fingerprint") != entry.get("environment_fingerprint"):
        raise SystemExit(f"{path.name}: capsule toolchain environment does not match its lock")
    toolchain_entries.append(entry)
for field in ("version", "libc", "compiler_sha256", "environment_fingerprint"):
    values = {entry.get(field) for entry in toolchain_entries}
    if len(values) != 1:
        raise SystemExit(f"{expected_toolchain} lock mismatch for {field}: {sorted(map(str, values))}")

baseline = capsules[0]
if any(capsule.get("repeatability", {}).get("runs") != 20 for capsule in capsules):
    raise SystemExit("each cross-machine capsule must contain exactly 20 samples")
if len(set(runner_names)) != 3:
    raise SystemExit(f"expected 3 independent runner labels, got {runner_names}")
fingerprints = []
for capsule, path in zip(capsules, capsule_paths):
    fingerprint = capsule.get("host", {}).get("fingerprint")
    if not isinstance(fingerprint, str) or not fingerprint:
        raise SystemExit(f"{path.name}: missing host fingerprint")
    fingerprints.append(fingerprint)
identity_fields = (
    ("schema_version",),
    ("problem", "id"),
    ("problem", "statement_sha256"),
    ("source", "source_sha256"),
    ("toolchain", "name"),
    ("toolchain", "version"),
    ("toolchain", "environment_fingerprint"),
    ("build", "binary_sha256"),
    ("build", "build_repeat"),
    ("build", "argv"),
    ("build", "opt_level"),
    ("build", "stdin_sha256"),
    ("reference_build", "binary_sha256"),
    ("reference_build", "argv"),
    ("harness", "version"),
    ("harness", "ruleset"),
    ("input", "generator"),
    ("input", "seed"),
    ("input", "seed_set"),
    ("input", "size"),
    ("verdict",),
)


def value_at(obj, path):
    for key in path:
        obj = obj[key]
    return obj


for capsule, path in zip(capsules[1:], capsule_paths[1:]):
    for field_path in identity_fields:
        try:
            expected = value_at(baseline, field_path)
            actual = value_at(capsule, field_path)
        except (KeyError, TypeError):
            raise SystemExit(f"{path.name}: missing capsule identity field {'.'.join(field_path)}")
        if actual != expected:
            raise SystemExit(f"{path.name}: capsule identity differs at {'.'.join(field_path)}")

metric_maps = []
for capsule, path in zip(capsules, capsule_paths):
    comparable_metric = lambda item: item["kind"] == "simulated" or (
        item["kind"] == "measured"
        and item["method"] in ("manual", "valgrind_memcheck", "stat", "runner")
    )
    metrics = {
        item["name"]: item
        for item in capsule["metrics"]
        if comparable_metric(item)
    }
    if len(metrics) != sum(comparable_metric(item) for item in capsule["metrics"]):
        raise SystemExit(f"{path.name}: duplicate deterministic metric name")
    metric_maps.append(metrics)

names = set(metric_maps[0])
if not names:
    raise SystemExit("capsules contain no deterministic metrics")
if any(set(metrics) != names for metrics in metric_maps[1:]):
    raise SystemExit("capsules contain different measured/simulated metric sets")

for name in sorted(names):
    entries = [metrics[name] for metrics in metric_maps]
    identity = {(entry["kind"], entry["method"], entry["unit"]) for entry in entries}
    if len(identity) != 1:
        raise SystemExit(f"metric metadata differs for {name}")
    values = [entry["value"] for entry in entries]
    if any(
        not isinstance(value, (int, float))
        or isinstance(value, bool)
        or not math.isfinite(value)
        or value < 0
        for value in values
    ):
        raise SystemExit(f"metric value is invalid for {name}: {values}")
    if len(set(values)) != 1:
        raise SystemExit(f"{name} differs across machines: {values}")
    spread = 0.0 if values[0] == 0 else (max(values) - min(values)) / abs(values[0]) * 100
    print(f"{name}: {values[0]} {entries[0]['unit']} ({entries[0]['kind']}, cross-host spread {spread:.4f}%)")

for capsule, path, runner, fingerprint, metrics in zip(
    capsules, capsule_paths, runner_names, fingerprints, metric_maps
):
    per_metric = capsule["repeatability"].get("per_metric", {})
    missing = names - set(per_metric)
    if missing:
        raise SystemExit(f"{path.name}: repeatability data is missing {sorted(missing)}")
    for name in sorted(names):
        try:
            spread = per_metric[name]
            minimum, maximum, median, reported = (
                float(spread[field]) for field in ("min", "max", "median", "spread_pct")
            )
        except (KeyError, TypeError, ValueError):
            raise SystemExit(f"{path.name}: invalid repeatability data for {name}")
        if not all(math.isfinite(value) for value in (minimum, maximum, median, reported)):
            raise SystemExit(f"{path.name}: non-finite repeatability data for {name}")
        if minimum < 0 or minimum > median or median > maximum or reported < 0:
            raise SystemExit(f"{path.name}: invalid repeatability range for {name}")
        actual = (maximum - minimum) / max(abs(median), 1.0) * 100
        if abs(actual - reported) > max(1e-9, abs(actual) * 1e-9):
            raise SystemExit(f"{path.name}: inconsistent repeatability spread for {name}")
        if actual > max_spread_pct:
            raise SystemExit(
                f"{path.name}: {name} within-host spread {actual:.6f}% exceeds "
                f"the {max_spread_pct:.6f}% gate"
            )
        if metrics[name]["value"] != median:
            raise SystemExit(f"{path.name}: {name} capsule value is not its 20-run median")
        print(
            f"{runner} ({fingerprint[:12]}): "
            f"{name} within-host spread {actual:.4f}%"
        )

print(f"host fingerprints recorded for 3 runner environments ({expected_toolchain})")
print(f"all deterministic metrics stayed within {max_spread_pct:.6f}% within-host spread")

import json
import pathlib
import shutil
import subprocess
import sys
import tempfile

ccx = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else "target/release/ccx").resolve()
repo = pathlib.Path(__file__).resolve().parents[1]

with tempfile.TemporaryDirectory(prefix="ccx-experiment-test-") as temporary:
    root = pathlib.Path(temporary)
    capsule = root / "accepted.json"
    result = subprocess.run(
        [
            str(ccx),
            "experiment",
            "--dir",
            str(repo / "arena/good"),
            "--size",
            "good",
            "--seed",
            "42",
            "--repeat",
            "2",
            "--cases",
            "10",
            "--jobs",
            "1",
            "--out",
            str(capsule),
            "--format",
            "json",
        ],
        capture_output=True,
        text=True,
        check=True,
    )
    report = json.loads(result.stdout)
    assert report["status"] == "accepted"
    assert report["correctness"]["overall"] == "accepted"
    assert report["correctness"]["complete"] is True
    assert report["correctness"]["generated_cases"] == 10
    assert report["measurement"]["capsule_id"]
    assert report["measurement"]["hints"] is not None
    assert any(metric["name"] == "instructions" for metric in report["measurement"]["metrics"])
    assert any(metric["name"] == "build_wall_clock" for metric in report["measurement"]["metrics"])
    assert report["assembly"]["assembly_files"]
    assert report["assembly"]["remarks_status"] in ("available", "unavailable")
    assert capsule.is_file()

    replay = subprocess.run(
        [str(ccx), "replay", str(capsule), "--dir", str(repo / "arena/good"), "--format", "json"],
        capture_output=True,
        text=True,
        check=True,
    )
    replay_report = json.loads(replay.stdout)
    assert replay_report["deterministic_match"] is True

    forked_solution = root / "forked-solution"
    forked_solution.mkdir()
    for name in ("ccx.toml", "main.cpp"):
        shutil.copyfile(repo / "arena/good" / name, forked_solution / name)
    config = forked_solution / "ccx.toml"
    config.write_text(
        config.read_text().replace(
            "../../problems/sorted-pair-sum",
            str(repo / "problems/sorted-pair-sum"),
        )
    )
    source = forked_solution / "main.cpp"
    source_text = source.read_text().replace(
        "#include <vector>",
        "#include <vector>\n#include <sys/wait.h>\n#include <unistd.h>",
    )
    source_text = source_text.replace(
        "int main() {",
        "int main() {\n"
        "    pid_t child = fork();\n"
        "    if (child == 0) {\n"
        "        execl(\"/bin/sh\", \"sh\", \"-c\", \"i=0; while [ $i -lt 1000 ]; do i=$((i+1)); done\", static_cast<char *>(nullptr));\n"
        "        _exit(127);\n"
        "    }\n"
        "    if (child < 0) return 2;\n"
        "    int status = 0;\n"
        "    if (waitpid(child, &status, 0) < 0 || !WIFEXITED(status) || WEXITSTATUS(status) != 0) return 3;",
    )
    source.write_text(source_text)
    forked_capsule = root / "forked-capsule.json"
    forked_measure = subprocess.run(
        [
            str(ccx),
            "measure",
            "--dir",
            str(forked_solution),
            "--size",
            "good",
            "--seed",
            "42",
            "--repeat",
            "2",
            "--out",
            str(forked_capsule),
            "--format",
            "json",
        ],
        capture_output=True,
        text=True,
        check=True,
    )
    forked_report = json.loads(forked_measure.stdout)
    regular_instructions = next(
        metric["value"]
        for metric in report["measurement"]["metrics"]
        if metric["name"] == "instructions"
    )
    forked_instructions = next(
        metric["value"]
        for metric in forked_report["metrics"]
        if metric["name"] == "instructions"
    )
    assert forked_instructions > regular_instructions
    assert json.loads(forked_capsule.read_text())["harness"]["version"] == "0.18.0"
    forked_replay = subprocess.run(
        [
            str(ccx),
            "replay",
            str(forked_capsule),
            "--dir",
            str(forked_solution),
            "--format",
            "json",
        ],
        capture_output=True,
        text=True,
        check=True,
    )
    assert json.loads(forked_replay.stdout)["deterministic_match"] is True

    zero_solution = root / "zero-allocation-solution"
    zero_solution.mkdir()
    shutil.copyfile(repo / "arena/good" / "ccx.toml", zero_solution / "ccx.toml")
    zero_config = zero_solution / "ccx.toml"
    zero_config.write_text(
        zero_config.read_text().replace(
            "../../problems/sorted-pair-sum",
            str(repo / "problems/sorted-pair-sum"),
        )
    )
    (zero_solution / "main.cpp").write_text(
        "#include <unistd.h>\n"
        "static long long values[1024];\n"
        "static long long next_number() {\n"
        "    char c = 0;\n"
        "    ssize_t count = 0;\n"
        "    do {\n"
        "        count = read(0, &c, 1);\n"
        "        if (count != 1) return 0;\n"
        "    } while (c <= 32);\n"
        "    long long number = 0;\n"
        "    while (c > 32) {\n"
        "        number = number * 10 + (c - '0');\n"
        "        count = read(0, &c, 1);\n"
        "        if (count != 1) break;\n"
        "    }\n"
        "    return number;\n"
        "}\n"
        "int main() {\n"
        "    long long n = next_number();\n"
        "    long long threshold = next_number();\n"
        "    for (long long i = 0; i < n; ++i) values[i] = next_number();\n"
        "    long long pairs = 0;\n"
        "    for (long long i = 0; i < n; ++i)\n"
        "        for (long long j = i + 1; j < n; ++j)\n"
        "            if (values[i] + values[j] <= threshold) ++pairs;\n"
        "    char digits[24];\n"
        "    int length = 0;\n"
        "    do {\n"
        "        digits[length++] = static_cast<char>('0' + pairs % 10);\n"
        "        pairs /= 10;\n"
        "    } while (pairs != 0);\n"
        "    char output[25];\n"
        "    for (int i = 0; i < length; ++i) output[i] = digits[length - i - 1];\n"
        "    output[length++] = '\\n';\n"
        "    return write(1, output, static_cast<unsigned long>(length)) == length ? 0 : 1;\n"
        "}\n"
    )
    zero_capsule = root / "zero-allocation-capsule.json"
    zero_measure = subprocess.run(
        [
            str(ccx),
            "measure",
            "--dir",
            str(zero_solution),
            "--size",
            "naive",
            "--seed",
            "42",
            "--repeat",
            "2",
            "--out",
            str(zero_capsule),
            "--format",
            "json",
        ],
        capture_output=True,
        text=True,
        check=True,
    )
    zero_report = json.loads(zero_measure.stdout)
    zero_metrics = {metric["name"]: metric for metric in zero_report["metrics"]}
    assert zero_metrics["allocations"]["value"] == 0
    assert zero_metrics["allocated_bytes"]["value"] == 0
    assert zero_metrics["allocations"]["kind"] == "measured"
    assert json.loads(zero_capsule.read_text())["harness"]["version"] == "0.18.0"
    zero_replay = subprocess.run(
        [
            str(ccx),
            "replay",
            str(zero_capsule),
            "--dir",
            str(zero_solution),
            "--format",
            "json",
        ],
        capture_output=True,
        text=True,
        check=True,
    )
    assert json.loads(zero_replay.stdout)["deterministic_match"] is True

    bad_solution = root / "bad-solution"
    subprocess.run(
        [
            str(ccx),
            "init",
            "--problem",
            str(repo / "problems/sorted-pair-sum"),
            "--language",
            "cpp",
            "--toolchain",
            "gcc",
            "--dir",
            str(bad_solution),
            "--format",
            "json",
        ],
        capture_output=True,
        text=True,
        check=True,
    )
    (bad_solution / "main.cpp").write_text("int main() { return 0; }\n")
    rejected = subprocess.run(
        [
            str(ccx),
            "experiment",
            "--dir",
            str(bad_solution),
            "--size",
            "good",
            "--seed",
            "42",
            "--repeat",
            "2",
            "--format",
            "json",
        ],
        capture_output=True,
        text=True,
    )
    assert rejected.returncode != 0
    rejected_report = json.loads(rejected.stdout)
    assert rejected_report["status"] == "correctness_failed"
    assert rejected_report["measurement"] is None

print("experiment, assembly, process-tree measurement, replay, and correctness gate passed")

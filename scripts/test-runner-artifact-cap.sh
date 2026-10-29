#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
test_dir="$(mktemp -d "${TMPDIR:-/tmp}/ccx-runner-artifact-test.XXXXXX")"
trap 'rm -rf "$test_dir"' EXIT

gcc -O2 -std=gnu11 -o "$test_dir/runner" "$repo_root/docker/harness/runner.c"
python3 - "$test_dir/runner" "$test_dir" "$repo_root" <<'PY'
import json
import os
import pathlib
import subprocess
import sys

runner = sys.argv[1]
root = pathlib.Path(sys.argv[2])
repo_root = pathlib.Path(sys.argv[3])
base = [
    runner,
    '--timeout-ms', '1000',
    '--memory-bytes', '536870912',
    '--stack-bytes', '8388608',
    '--pids-limit', '64',
    '--max-output-bytes', '1024',
]

def invoke(artifact):
    return subprocess.run(
        base + ['--emit-file', str(artifact), '--', '/bin/true'],
        capture_output=True,
        check=False,
        timeout=3,
    )

def invoke_program(program):
    return subprocess.run(
        base + ['--', str(program)],
        capture_output=True,
        check=False,
        timeout=3,
    )

def invoke_program_with_limit(program, output_limit):
    arguments = base.copy()
    arguments[arguments.index('--max-output-bytes') + 1] = str(output_limit)
    return subprocess.run(
        arguments + ['--', str(program)],
        capture_output=True,
        check=False,
        timeout=3,
    )

def invoke_args(arguments):
    return subprocess.run(
        arguments,
        capture_output=True,
        check=False,
        timeout=3,
    )

def parse_report(data):
    cursor = 0
    sections = {}
    for label in (b'CCX-OUT', b'CCX-ERR', b'CCX-ALLOC', b'CCX-EMIT'):
        end = data.index(b'\n', cursor)
        header = data[cursor:end].split()
        assert header[0] == label
        length = int(header[1])
        cursor = end + 1
        sections[label] = data[cursor:cursor + length]
        assert len(sections[label]) == length
        cursor += length
    verdict = data[cursor:].split(b'CCX-VERDICT ', 1)[1].splitlines()[0]
    return sections, json.loads(verdict)

small = root / 'small.bin'
small.write_bytes(b'hello')
result = invoke(small)
assert result.returncode == 0
assert b'CCX-EMIT 5\nhelloCCX-VERDICT ' in result.stdout
verdict = json.loads(result.stdout.split(b'CCX-VERDICT ', 1)[1].splitlines()[0])
assert verdict['artifact_too_large'] is False

boundary = root / 'boundary.bin'
with boundary.open('wb') as stream:
    stream.truncate(64 * 1024 * 1024)
result = invoke(boundary)
assert result.returncode == 0
emit_header = b'CCX-EMIT 67108864\n'
emit_start = result.stdout.index(emit_header) + len(emit_header)
emit_end = emit_start + 64 * 1024 * 1024
assert result.stdout[emit_end:].startswith(b'CCX-VERDICT ')
verdict = json.loads(result.stdout.split(b'CCX-VERDICT ', 1)[1].splitlines()[0])
assert verdict['artifact_too_large'] is False

oversized = root / 'oversized.bin'
with oversized.open('wb') as stream:
    stream.truncate(64 * 1024 * 1024 + 1)
result = invoke(oversized)
assert result.returncode == 125
assert b'CCX-EMIT 0\n' in result.stdout
assert len(result.stdout) < 4096
verdict = json.loads(result.stdout.split(b'CCX-VERDICT ', 1)[1].splitlines()[0])
assert verdict['artifact_too_large'] is True

symlink = root / 'artifact-link'
symlink.symlink_to(oversized)
result = invoke(symlink)
assert result.returncode == 0
assert b'CCX-EMIT 0\n' in result.stdout
assert len(result.stdout) < 4096
verdict = json.loads(result.stdout.split(b'CCX-VERDICT ', 1)[1].splitlines()[0])
assert verdict['artifact_too_large'] is False

fifo = root / 'artifact.fifo'
os.mkfifo(fifo)
result = invoke(fifo)
assert result.returncode == 0
assert b'CCX-EMIT 0\n' in result.stdout
assert len(result.stdout) < 4096
verdict = json.loads(result.stdout.split(b'CCX-VERDICT ', 1)[1].splitlines()[0])
assert verdict['artifact_too_large'] is False

stdout_source = b'#include <stdio.h>\nint main(void) { for (int i = 0; i < 600; i++) putchar(120); return 0; }\n'
stdout_program = root / 'stdout-under-total-cap'
subprocess.run(['gcc', '-O2', '-x', 'c', '-o', str(stdout_program), '-'], input=stdout_source, check=True)
result = invoke_program(stdout_program)
sections, verdict = parse_report(result.stdout)
assert result.returncode == 0
assert len(sections[b'CCX-OUT']) == 600
assert verdict['stdout_bytes'] == 600
assert verdict['output_truncated'] is False

large_stdout_source = b'#include <stdio.h>\nint main(void) { for (int i = 0; i < 700000; i++) putchar(120); return 0; }\n'
large_stdout_program = root / 'stdout-multichunk-under-total-cap'
subprocess.run(['gcc', '-O2', '-x', 'c', '-o', str(large_stdout_program), '-'], input=large_stdout_source, check=True)
result = invoke_program_with_limit(large_stdout_program, 1048576)
sections, verdict = parse_report(result.stdout)
assert result.returncode == 0
assert len(sections[b'CCX-OUT']) == 700000
assert verdict['stdout_bytes'] == 700000
assert verdict['output_truncated'] is False

oversized_output_source = b'#include <stdio.h>\nint main(void) { for (int i = 0; i < 1100; i++) putchar(120); return 0; }\n'
oversized_output_program = root / 'stdout-over-total-cap'
subprocess.run(['gcc', '-O2', '-x', 'c', '-o', str(oversized_output_program), '-'], input=oversized_output_source, check=True)
result = invoke_program(oversized_output_program)
sections, verdict = parse_report(result.stdout)
assert result.returncode == 125
assert len(sections[b'CCX-OUT']) == 1024
assert verdict['stdout_bytes'] > len(sections[b'CCX-OUT'])
assert verdict['output_truncated'] is True

split_source = b'#include <stdio.h>\nint main(void) { for (int i = 0; i < 400; i++) putchar(120); for (int i = 0; i < 400; i++) fputc(121, stderr); return 0; }\n'
split_program = root / 'split-output'
subprocess.run(['gcc', '-O2', '-x', 'c', '-o', str(split_program), '-'], input=split_source, check=True)
result = invoke_program(split_program)
sections, verdict = parse_report(result.stdout)
assert result.returncode == 0
assert len(sections[b'CCX-OUT']) == 400
assert len(sections[b'CCX-ERR']) == 400
assert verdict['output_truncated'] is False

split_over_source = b'#include <stdio.h>\nint main(void) { for (int i = 0; i < 700; i++) putchar(120); for (int i = 0; i < 700; i++) fputc(121, stderr); return 0; }\n'
split_over_program = root / 'split-over-total-cap'
subprocess.run(['gcc', '-O2', '-x', 'c', '-o', str(split_over_program), '-'], input=split_over_source, check=True)
result = invoke_program(split_over_program)
sections, verdict = parse_report(result.stdout)
assert result.returncode == 125
assert len(sections[b'CCX-OUT']) + len(sections[b'CCX-ERR']) == 1024
assert verdict['stdout_bytes'] + verdict['stderr_bytes'] > 1024
assert verdict['output_truncated'] is True

for option, invalid in (
    ('--timeout-ms', '0'),
    ('--timeout-ms', '10ms'),
    ('--memory-bytes', '-1'),
    ('--stack-bytes', '0'),
    ('--pids-limit', '0'),
    ('--max-output-bytes', '1'),
):
    arguments = base.copy()
    arguments[arguments.index(option) + 1] = invalid
    result = invoke_args(arguments + ['--', '/bin/true'])
    assert result.returncode == 2
    assert b'invalid ' + option.encode() + b' value' in result.stderr
    assert b'CCX-VERDICT ' not in result.stdout

limit_source = b'#include <stdio.h>\n#include <sys/resource.h>\nint main(void) { struct rlimit a, s, p, c; if (getrlimit(RLIMIT_AS, &a) || getrlimit(RLIMIT_STACK, &s) || getrlimit(RLIMIT_NPROC, &p) || getrlimit(RLIMIT_CORE, &c)) return 1; printf("%lu %lu %lu %lu", (unsigned long)a.rlim_cur, (unsigned long)s.rlim_cur, (unsigned long)p.rlim_cur, (unsigned long)c.rlim_cur); return 0; }\n'
limit_program = root / 'resource-limits'
subprocess.run(['gcc', '-O2', '-x', 'c', '-o', str(limit_program), '-'], input=limit_source, check=True)
result = invoke_program(limit_program)
sections, verdict = parse_report(result.stdout)
assert result.returncode == 0
assert sections[b'CCX-OUT'] == b'536870912 8388608 64 0'
assert verdict['output_truncated'] is False

deny_source = b'#include <errno.h>\n#include <sys/resource.h>\nint ccx_test_setrlimit(int resource, const struct rlimit *limit) { (void)resource; (void)limit; errno = EPERM; return -1; }\n'
deny_file = root / 'deny-setrlimit.c'
deny_file.write_bytes(deny_source)
deny_runner = root / 'runner-deny-setrlimit'
subprocess.run(['gcc', '-O2', '-std=gnu11', '-Dsetrlimit=ccx_test_setrlimit', '-o', str(deny_runner), str(repo_root / 'docker/harness/runner.c'), str(deny_file)], check=True)
result = subprocess.run([str(deny_runner)] + base[1:] + ['--', '/bin/true'], capture_output=True, check=False, timeout=3)
sections, verdict = parse_report(result.stdout)
assert result.returncode == 2
assert verdict['exit_status'] == 2
assert b'cannot apply address-space limit' in sections[b'CCX-ERR']

print('artifact transfer, size cap, symlink rejection, and FIFO rejection passed')
print('single-stream and split-stream output within the total cap is preserved; excess output is truncated')
print('invalid runner limits are rejected and resource limits are applied to the child')
print('the runner refuses to execute when a required resource limit cannot be applied')
PY

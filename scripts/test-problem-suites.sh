#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ccx_bin="${CCX_BIN:-$repo_root/target/release/ccx}"
test_root="$(mktemp -d "${TMPDIR:-/tmp}/ccx-problem-suites.XXXXXX")"
trap 'rm -rf "$test_root"' EXIT

problems=(
  "sorted-pair-sum"
  "data-structures/top-k"
  "parsing/csv-sum"
  "queues/fifo-stream"
  "memory/allocation-layout"
)

for problem in "${problems[@]}"; do
  name="${problem##*/}"
  solution="$test_root/$name"
  "$ccx_bin" init \
    --problem "$repo_root/problems/$problem" \
    --language cpp \
    --toolchain gcc \
    --dir "$solution" \
    --format json
  cp "$repo_root/problems/$problem/solutions/good.cpp" "$solution/main.cpp"
  "$ccx_bin" test --dir "$solution" --hidden --format json
done

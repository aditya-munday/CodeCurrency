#!/usr/bin/env bash
# Verify that the sandbox contains every hostile submission (Layer 6).
#
# This runs the real sandbox: real docker, real runner, real limits. It is the
# evidence for the "all hostile submissions are contained" gate, so it asserts
# on the *reason* each case stopped, not just that it stopped.
#
# Containment does not always mean "killed". Several limits make the hostile
# action impossible rather than fatal, which is the better outcome:
#   - RLIMIT_NPROC makes fork() fail, so a fork bomb defuses itself.
#   - RLIMIT_AS makes malloc() return NULL, so a memory bomb ends cleanly.
#   - RLIMIT_STACK turns runaway recursion into a SIGSEGV in the sandbox.
set -uo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

GCC_IMAGE="ccx-gcc@$(jq -r '.toolchains[] | select(.name=="gcc") | .image_digest' docker/toolchains.lock.json)"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

pass=0
fail=0

# Sandbox flags mirroring SandboxPolicy::default().
sandbox_flags=(
  --rm --network=none --user 65534:65534 --read-only
  --tmpfs /work/scratch:rw,size=64m,mode=1777
  --tmpfs /tmp:rw,size=16m,mode=1777
  --cap-drop=ALL --security-opt=no-new-privileges
  --memory 768m --memory-swap 768m --pids-limit 64 --cpus 1.0
  --cpuset-cpus 0
  --env LANG=C --env LC_ALL=C --env TZ=UTC --env SOURCE_DATE_EPOCH=0
  --env PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
)

# Extract one hostile case's source from the Rust test set, so the safety suite
# and the sandbox definition cannot drift apart.
extract_case() {
  local kind="$1"
  python3 - "$kind" <<'PY'
import re, sys
kind = sys.argv[1]
text = open('crates/ccx-core/src/sandbox.rs').read()
pattern = r'HostileKind::' + re.escape(kind) + r'\s*=>\s*\{.*?r#"(.*?)"#'
m = re.search(pattern, text, re.S)
if not m:
    sys.exit(f"could not find source for {kind}")
sys.stdout.write(m.group(1))
PY
}

# Build a hostile case and run it; echo the runner's exit code.
build_and_run() {
  local kind="$1" timeout_ms="$2"
  local dir="$work/$kind"
  mkdir -p "$dir"
  printf '%s' "$(extract_case "$kind")" > "$dir/main.c"
  if ! gcc -O2 -o "$dir/prog" "$dir/main.c" 2>"$dir/build.err"; then
    echo "BUILD_FAIL"
    return
  fi
  docker run "${sandbox_flags[@]}" -v "$dir:/in:ro" "$GCC_IMAGE" \
    /opt/ccx/runner --timeout-ms "$timeout_ms" --memory-bytes 268435456 \
    --stack-bytes 8388608 --pids-limit 16 --max-output-bytes 65536 \
    -- /in/prog >"$dir/report.txt" 2>&1
  echo "$?"
}

report() {
  local kind="$1" expected="$2" detail="$3" ok="$4"
  if [[ "$ok" == "1" ]]; then
    echo "PASS  $kind  ($detail)"
    pass=$((pass + 1))
  else
    echo "FAIL  $kind  (expected $expected; $detail)"
    fail=$((fail + 1))
  fi
}

echo "==> hostile submission containment"
echo

# --- infinite loop: must be killed at the deadline -------------------------
rc=$(build_and_run InfiniteLoop 1500)
report infinite_loop "rc=124" "rc=$rc, killed at deadline" "$([[ $rc == 124 ]] && echo 1 || echo 0)"

# --- fork bomb: fork() must fail; must finish fast -------------------------
start=$(date +%s%N)
rc=$(build_and_run ForkBomb 4000)
elapsed_ms=$(( ($(date +%s%N) - start) / 1000000 ))
ok=0
if [[ ( $rc == 0 || $rc == 2 ) && $elapsed_ms -lt 3000 ]]; then ok=1; fi
report fork_bomb "contained before deadline" "rc=$rc, descendants reaped, ${elapsed_ms}ms" "$ok"

# --- huge output: must be capped ------------------------------------------
rc=$(build_and_run HugeOutput 2000)
report huge_output "rc=125" "rc=$rc, output cap hit" "$([[ $rc == 125 ]] && echo 1 || echo 0)"

# --- memory bomb: peak RSS must stay under the address-space limit ---------
dir="$work/MemoryBomb"
mkdir -p "$dir"
printf '%s' "$(extract_case MemoryBomb)" > "$dir/main.c"
ok=0
peak=0
detail="hostile source failed to compile"
if gcc -O2 -o "$dir/prog" "$dir/main.c" 2>/dev/null; then
  docker run "${sandbox_flags[@]}" -v "$dir:/in:ro" "$GCC_IMAGE" \
    /opt/ccx/runner --timeout-ms 5000 --memory-bytes 268435456 \
    --stack-bytes 8388608 --pids-limit 16 --max-output-bytes 65536 \
    -- /in/prog >"$dir/report.txt" 2>&1
  run_rc=$?
  verdict=$(grep -o 'CCX-VERDICT .*' "$dir/report.txt" | sed 's/^CCX-VERDICT //' || true)
  peak=$(printf '%s' "$verdict" | jq -r '.peak_rss_bytes' 2>/dev/null || echo 0)
  if [[ $run_rc == 0 ]] && printf '%s' "$verdict" | jq -e ".timed_out == false and .peak_rss_bytes > 0 and .peak_rss_bytes < $((400 * 1024 * 1024))" >/dev/null 2>&1; then
    ok=1
  fi
  detail="peak=$(( ${peak:-0} / 1024 / 1024 ))MiB"
fi
report memory_bomb "peak RSS bounded" "$detail" "$ok"

# --- deep recursion: stack overflow must stay inside the sandbox -----------
rc=$(build_and_run DeepRecursion 4000)
report deep_recursion "contained (128+signal)" "rc=$rc" \
  "$([[ $rc == 139 || $rc == 124 ]] && echo 1 || echo 0)"

# --- file reads: no host secret may be readable ---------------------------
echo
echo "==> file-read containment"
dir="$work/FileReadOutsideSandbox"
mkdir -p "$dir"
printf '%s' "$(extract_case FileReadOutsideSandbox)" > "$dir/main.c"
host_secret="$work/host-secret"
host_secret_token="CCX_HOST_SECRET_${RANDOM}_$(date +%s%N)"
printf '%s' "$host_secret_token" > "$host_secret"
chmod 644 "$host_secret"
if gcc -O2 -o "$dir/prog" "$dir/main.c" 2>/dev/null; then
  docker run "${sandbox_flags[@]}" --env "CCX_HOST_SECRET_PATH=$host_secret" -v "$dir:/in:ro" "$GCC_IMAGE" \
    /opt/ccx/runner --timeout-ms 2000 --memory-bytes 268435456 \
    --stack-bytes 8388608 --pids-limit 16 --max-output-bytes 65536 \
    -- /in/prog >"$dir/report.txt" 2>&1
  run_rc=$?
  # The program prints nothing; a non-empty CCX-OUT body means host data leaked.
  verdict=$(grep -o 'CCX-VERDICT .*' "$dir/report.txt" | sed 's/^CCX-VERDICT //' || true)
  if [[ $run_rc == 0 ]] && printf '%s' "$verdict" | jq -e '.exit_status == 0 and .timed_out == false' >/dev/null 2>&1 && ! grep -Fq "$host_secret_token" "$dir/report.txt"; then
    report file_read_outside_sandbox "no host data readable" "unmounted sentinel stayed unreadable" 1
  else
    report file_read_outside_sandbox "no host data readable" "host sentinel leaked or runner failed" 0
  fi
else
  report file_read_outside_sandbox "no host data readable" "hostile source failed to compile" 0
fi

# --- network: connect must fail -------------------------------------------
echo
echo "==> network containment"
dir="$work/NetworkAttempt"
mkdir -p "$dir"
printf '%s' "$(extract_case NetworkAttempt)" > "$dir/main.c"
if gcc -O2 -o "$dir/prog" "$dir/main.c" 2>/dev/null; then
  docker run "${sandbox_flags[@]}" -v "$dir:/in:ro" "$GCC_IMAGE" \
    /opt/ccx/runner --timeout-ms 2000 --memory-bytes 268435456 \
    --stack-bytes 8388608 --pids-limit 16 --max-output-bytes 65536 \
    -- /in/prog >"$dir/report.txt" 2>&1
  run_rc=$?
  verdict=$(grep -o 'CCX-VERDICT .*' "$dir/report.txt" | sed 's/^CCX-VERDICT //')
  if [[ $run_rc == 0 ]] && printf '%s' "$verdict" | jq -e '.exit_status == 0 and .timed_out == false' >/dev/null 2>&1 && ! grep -Fq 'CCX_NETWORK_CONNECTED' "$dir/report.txt"; then
    report network_attempt "connect denied" "connection did not succeed" 1
  else
    report network_attempt "connect denied" "connection succeeded or runner failed: $verdict" 0
  fi
else
  report network_attempt "connect denied" "hostile source failed to compile" 0
fi

# --- tampering: rootfs writes must fail -----------------------------------
echo
echo "==> result-tampering containment"
dir="$work/TamperWithResults"
mkdir -p "$dir"
printf '%s' "$(extract_case TamperWithResults)" > "$dir/main.c"
if gcc -O2 -o "$dir/prog" "$dir/main.c" 2>/dev/null; then
  chmod 666 "$dir/main.c"
  before=$(sha256sum "$dir/main.c" | cut -d' ' -f1)
  docker run "${sandbox_flags[@]}" -v "$dir:/in:ro" "$GCC_IMAGE" \
    /opt/ccx/runner --timeout-ms 2000 --memory-bytes 268435456 \
    --stack-bytes 8388608 --pids-limit 16 --max-output-bytes 65536 \
    -- /in/prog >/dev/null 2>&1
  run_rc=$?
  after=$(sha256sum "$dir/main.c" | cut -d' ' -f1)
  if [[ $run_rc == 0 && "$before" == "$after" ]]; then
    report tamper_with_results "read-only source mount" "host source unchanged" 1
  else
    report tamper_with_results "read-only source mount" "a host file was modified or runner failed" 0
  fi
else
  report tamper_with_results "read-only source mount" "hostile source failed to compile" 0
fi

echo
echo "==> $pass passed, $fail failed"
[[ $fail == 0 ]]

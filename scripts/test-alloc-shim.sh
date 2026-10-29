set -eu
repo_root="$(cd "$(dirname "$0")/.." && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
gcc -O2 -fno-builtin-malloc -fno-builtin-calloc -fno-builtin-realloc -fno-builtin-free -std=gnu11 -pthread -o "$work/test" \
    "$repo_root/docker/harness/alloc_shim_test.c" \
    "$repo_root/docker/harness/alloc_shim.c" \
    -Wl,--wrap=malloc,--wrap=calloc,--wrap=realloc,--wrap=free \
    -Wl,--wrap=posix_memalign,--wrap=aligned_alloc,--wrap=memalign
CCX_ALLOC_OUT="$work/alloc.json" "$work/test"
jq -e '.allocations > 8000 and .allocated_bytes > 0 and .frees > 8000 and .peak_live_bytes > 0' \
    "$work/alloc.json" >/dev/null

chmod 755 "$work"
cat > "$work/xtree-probe.c" <<'C'
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
int main(void) {
    char path[128];
    snprintf(path, sizeof path, "/work/scratch/alloc-%ld.xtree", (long)getpid());
    FILE *f = fopen(path, "w");
    if (f) {
        fputs("forged-allocation-report", f);
        fclose(f);
    }
    void *blocks[32];
    for (int i = 0; i < 32; i++) {
        blocks[i] = malloc((size_t)(i + 1) * 64);
        if (!blocks[i]) {
            return 3;
        }
    }
    for (int i = 0; i < 32; i++) {
        free(blocks[i]);
    }
    return 0;
}
C
gcc -O2 -o "$work/xtree-probe" "$work/xtree-probe.c"
image="ccx-gcc@$(jq -r '.toolchains[] | select(.name=="gcc") | .image_digest' "$repo_root/docker/toolchains.lock.json")"
docker_args=(
    --rm --network=none --user 65534:65534 --read-only
    --tmpfs /work/scratch:rw,size=64m,mode=1777
    --tmpfs /tmp:rw,size=16m,mode=1777
    --cap-drop=ALL --security-opt=no-new-privileges
    --memory 2g --memory-swap 2g --pids-limit 64 --cpus 1.0 --cpuset-cpus 0
    -v "$work:/in:ro" "$image"
)
docker run "${docker_args[@]}" \
    /opt/ccx/runner --timeout-ms 12000 --memory-bytes 2147483648 \
    --stack-bytes 8388608 --pids-limit 16 --max-output-bytes 65536 \
    --alloc-xtree /work/scratch/alloc-%p.xtree -- \
    /usr/bin/valgrind --tool=memcheck --xtree-memory=full \
    --xtree-memory-file=/work/scratch/alloc-%p.xtree /in/xtree-probe \
    > "$work/xtree-report.bin" 2>&1
python3 - "$work/xtree-report.bin" <<'PY'
import json, pathlib, sys
raw = pathlib.Path(sys.argv[1]).read_bytes()
pos = 0
def line():
    global pos
    end = raw.index(b"\n", pos)
    result = raw[pos:end].decode()
    pos = end + 1
    return result
def section(tag):
    global pos
    size = int(line().removeprefix(tag))
    body = raw[pos:pos + size]
    if len(body) != size:
        raise SystemExit(f"truncated {tag} section")
    pos += size
    return body
section("CCX-OUT ")
section("CCX-ERR ")
allocations = section("CCX-ALLOC ")
section("CCX-EMIT ")
verdict = json.loads(line().removeprefix("CCX-VERDICT "))
if b"forged-allocation-report" in allocations:
    raise SystemExit("the runner accepted a submission-written allocation report")
counts = json.loads(allocations)
if verdict["exit_status"] != 0:
    raise SystemExit(f"the clean allocation probe did not exit successfully: {verdict}")
if counts["allocations"] < 32 or counts["allocated_bytes"] < 33792:
    raise SystemExit(f"Memcheck totals omit the test allocations: {counts}")
PY
cat > "$work/empty-xtree-probe.c" <<'C'
int main(void) {
    return 0;
}
C
gcc -O2 -o "$work/empty-xtree-probe" "$work/empty-xtree-probe.c"
docker run "${docker_args[@]}" \
    /opt/ccx/runner --timeout-ms 12000 --memory-bytes 2147483648 \
    --stack-bytes 8388608 --pids-limit 16 --max-output-bytes 65536 \
    --alloc-xtree /work/scratch/alloc-%p.xtree -- \
    /usr/bin/valgrind --tool=memcheck --xtree-memory=full \
    --xtree-memory-file=/work/scratch/alloc-%p.xtree /in/empty-xtree-probe \
    > "$work/empty-xtree-report.bin" 2>&1
python3 - "$work/empty-xtree-report.bin" <<'PY'
import json, pathlib, sys
raw = pathlib.Path(sys.argv[1]).read_bytes()
pos = 0
def line():
    global pos
    end = raw.index(b"\n", pos)
    result = raw[pos:end].decode()
    pos = end + 1
    return result
def section(tag):
    global pos
    size = int(line().removeprefix(tag))
    body = raw[pos:pos + size]
    if len(body) != size:
        raise SystemExit(f"truncated {tag} section")
    pos += size
    return body
section("CCX-OUT ")
section("CCX-ERR ")
allocations = section("CCX-ALLOC ")
section("CCX-EMIT ")
verdict = json.loads(line().removeprefix("CCX-VERDICT "))
counts = json.loads(allocations)
if verdict["exit_status"] != 0:
    raise SystemExit(f"the empty allocation probe did not exit successfully: {verdict}")
if counts != {"allocations": 0, "allocated_bytes": 0, "frees": 0}:
    raise SystemExit(f"the empty Memcheck profile was not reported as zero: {counts}")
PY
cat > "$work/orphan-probe.c" <<'C'
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/types.h>
#include <unistd.h>
int main(void) {
    pid_t parent = getpid();
    pid_t child = fork();
    if (child == 0) {
        close(1);
        close(2);
        usleep(100000);
        char path[128];
        snprintf(path, sizeof path, "/work/scratch/alloc-%ld.xtree", (long)parent);
        int fd = open(path, O_WRONLY | O_TRUNC | O_CREAT, 0666);
        if (fd >= 0) {
            if (write(fd, "forged-after-exit", 17) != 17) {
                close(fd);
                _exit(4);
            }
            close(fd);
        }
        _exit(0);
    }
    if (child < 0) {
        return 3;
    }
    return 0;
}
C
gcc -O2 -o "$work/orphan-probe" "$work/orphan-probe.c"
set +e
docker run "${docker_args[@]}" \
    /opt/ccx/runner --timeout-ms 12000 --memory-bytes 2147483648 \
    --stack-bytes 8388608 --pids-limit 16 --max-output-bytes 65536 \
    --alloc-xtree /work/scratch/alloc-%p.xtree -- \
    /usr/bin/valgrind --tool=memcheck --xtree-memory=full \
    --xtree-memory-file=/work/scratch/alloc-%p.xtree /in/orphan-probe \
    > "$work/orphan-report.bin" 2>&1
orphan_runner_status=$?
set -e
python3 - "$work/orphan-report.bin" "$orphan_runner_status" <<'PY'
import json, pathlib, sys
raw = pathlib.Path(sys.argv[1]).read_bytes()
runner_status = int(sys.argv[2])
pos = 0
def line():
    global pos
    end = raw.index(b"\n", pos)
    result = raw[pos:end].decode()
    pos = end + 1
    return result
def section(tag):
    global pos
    size = int(line().removeprefix(tag))
    body = raw[pos:pos + size]
    if len(body) != size:
        raise SystemExit(f"truncated {tag} section")
    pos += size
    return body
section("CCX-OUT ")
section("CCX-ERR ")
allocations = section("CCX-ALLOC ")
section("CCX-EMIT ")
verdict = json.loads(line().removeprefix("CCX-VERDICT "))
if b"forged-after-exit" in allocations:
    raise SystemExit("a detached child replaced the allocation profile")
if runner_status != verdict["exit_status"]:
    raise SystemExit(f"runner exit status disagrees with its report: {runner_status} {verdict}")
if runner_status not in (0, 2, 124):
    raise SystemExit(f"unexpected status for the forked tamper probe: {verdict}")
PY

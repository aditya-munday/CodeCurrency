# Measurement methodology

Every metric is stored with a `kind` and a `method`. The kind describes what a
number means; the method identifies the instrument that produced it.
`measured` means instrumentation read a real counter, not that the value is
stable across machines or protected from a hostile process; Linux `getrusage`
counters carry process and host variation.

| Metric | Kind | Method | Interpretation |
| --- | --- | --- | --- |
| `instructions`, cache misses, branch misses, memory references, branch instructions, and miss rates | simulated | Cachegrind | Deterministic model counts for the same binary and input; every run pins I1/D1 to 32 KiB, 8-way, 64-byte lines; cache misses are the sum of I1 and D1 misses, while host-detected LL misses are not emitted |
| `allocations`, `allocated_bytes` | measured | Valgrind Memcheck xtree | Main process heap blocks and requested bytes; forked and exec'd child profiles are not aggregated |
| `binary_size` | measured | filesystem stat | Size of the rebuilt executable |
| `peak_memory` | indicative | `getrusage` | Host and runtime dependent peak resident memory |
| `wall_clock` | indicative | runner monotonic clock | Candidate process wall time including output collection |
| `reference_wall_clock` | indicative | runner monotonic clock | C++ `reference.cpp` process wall time paired with the candidate |
| `paired_speedup_ratio` | indicative | paired wall-clock samples | Per-pair reference duration divided by candidate duration; above 1 means the candidate was faster on that pair |
| `stdout_bytes`, `stderr_bytes` | measured | sandbox runner | Exact bytes emitted by the child process |
| `user_cpu_time`, `system_cpu_time` | measured | `getrusage` | CPU time charged to the executed process |
| `minor_page_faults`, `major_page_faults` | measured | `getrusage` | Process page-fault counts |
| `voluntary_context_switches`, `involuntary_context_switches` | measured | `getrusage` | Process scheduler switch counts |
| `block_input_ops`, `block_output_ops` | measured | `getrusage` | Linux block-I/O operation counts, not byte totals |
| `instructions_hw`, `cache_misses_hw`, `branch_misses_hw`, `cpu_cycles_hw` | measured when available | Linux `perf_event_open` | Host hardware events; multiplexed counts are time-scaled, and unavailable events are omitted |
| `ipc_hw`, `average_cpu_frequency_hw` | measured when available | derived from `perf_event_open` and `getrusage` | Instructions per cycle and effective average frequency during user CPU time; host-specific |
| `build_wall_clock` | measured | runner monotonic clock | Median across three clean builds with identical executable bytes; host-dependent, with observed spread and bootstrap interval |
| `build_user_cpu_time`, `build_system_cpu_time` | measured | `getrusage` | Median across three clean builds with identical executable bytes; host-dependent, with observed spread and bootstrap interval |

`ccx measure --repeat N` checks the answer, performs three clean builds, then
measures that exact executable for every repetition. Each candidate wall-clock
sample is paired with a fresh run of the problem's C++ `reference.cpp` on the
same input. The pair runs consecutively and alternates order between samples to
reduce systematic warm-up and host-drift bias. The capsule records both wall
times and the median and confidence interval of the per-pair reference/candidate
ratio. Ratios above one favour the candidate. Schema 13 also stores the
reference compiler argv and executable SHA-256; replay rejects a changed
reference build before comparing timing. For Rust, D, Zig, Odin, and Go,
the reference remains C++; those ratios provide cross-language context and are
not a within-language ranking signal. The three candidate builds must produce
identical executable bytes; build time is reported as the median with observed
spread and a seeded bootstrap interval. Paired timing does not remove all noise
or make wall time a deterministic ranking metric. Replay repeats the recorded
number of clean builds, checks the solution and reference executable digests,
and repeats the same alternating paired run sequence. The runner captures each
build's wall time and OS CPU time while the compiler or build driver runs.
Docker startup and copying the binary to the host are excluded from
`build_wall_clock`. Build timings are measured host observations; three samples
expose only a small repeatability distribution and do not establish cross-host
stability. Cachegrind and native execution measurements are collected on each
repetition. Before sampling, one unrecorded fresh-process correctness probe
validates the output and acts as a warm-up. The capsule stores each metric's
median, observed p95 and p99, and
`(max - min) / max(abs(median), 1)` spread; the one-unit floor keeps near-zero
OS counters finite. When at least two samples exist it also stores a seeded
bootstrap 95% confidence interval of the median. The default is twenty runs,
matching the Phase 1 local repeatability sample size.

The measurement table and JSON report include the execution target, host
fingerprint, pinned toolchain environment fingerprint, and each metric's
observed minimum, maximum, p95, p99, and relative spread. Tail percentiles are
nearest-rank summaries of this sample, not confidence bounds; at twenty runs,
p99 is effectively the observed maximum. The uncertainty kind is
explicit in JSON; the human table labels its half-width as a 95% confidence
interval. Ratios and instructions per cycle retain fractional precision in the
human table. A caller that explicitly requests fewer than twenty runs gets the
actual sample count and range, not a claim that the smaller sample is equally
strong evidence.

Replay repeats the capsule's candidate build count, requires identical rebuilt
candidate bytes, verifies both executable digests and the reference compiler
argv, repeats the capsule's run count against those bytes, and compares the
resulting medians using twice the combined measured 95% confidence half-width. Zero uncertainty
requires an exact match. A match inside this tolerance means statistical
repeatability, not identical raw counts.
Build wall time and build CPU time are compared with the recorded build
interval and remain machine-dependent; they do not affect `deterministic_match`.
The harness version is recorded so replay can surface changes to these rules.
Harness version 0.14.0 adds three byte-identical clean-build samples and their
timing distributions. Harness version 0.15.0 rejects malformed runner limits,
fails closed when a resource limit cannot be applied. Harness version 0.16.0
grows output capture within the combined stdout and stderr limit, preserving a
single stream below the total allowance and marking output that exceeds it.
Harness version 0.17.0 requires complete per-process Cachegrind summaries and
sums counters for traced child processes. Harness version 0.13.0 pins the I1
and D1 cache geometry and adds alternating candidate/reference wall-clock
samples plus reference executable identity checks; Cachegrind 3.24 uses the
host-detected last-level cache despite the explicit LL option, and LL misses
are excluded from reported metrics. Earlier harness versions used the host CPU's
detected I1 and D1 cache sizes, so their simulated cache-miss values are not
cross-machine comparable.

Harness version 0.18.0 records a structurally complete Memcheck profile with
`totals: (null)` as zero allocations and bytes. Missing or malformed totals
fail measurement rather than becoming zeros.

`ccx matrix` measures each selected compiler, optimization, build-system, and
CMake-generator profile on the same problem, generated seed, and configured
input size. Every profile is built and measured independently and gets its own
capsule. Cachegrind metrics can be compared across profiles; OS and wall-clock
metrics remain specific to the host and run. The command compares only the
compiler versions already pinned in the toolchain lock. Replay discovers a
capsule's solution from its parent tree, or accepts `--dir` when the capsule was
saved elsewhere. Input hashes are checked across profiles before comparison;
profiles generated from different input bytes are never compared. Capsule
filenames include their content identity, so later matrix runs preserve earlier
evidence.
The combined report labels pairwise ranked-metric differences within twice the
combined 95% confidence half-width as `within_noise`; it is a same-host
comparison aid, not a cross-host significance test or ranking.
The measurement report marks a ranked metric eligible only when it is
machine-independent and its observed spread is at most 0.01%. Matrix pairwise
comparisons are withheld when either profile fails that check. This is a
same-host repeatability gate; cross-host ranking still requires the separate
multi-runner equality check, and the eligibility label does not make wall time
or hardware counters comparable between machines.

`ccx diff` calls metric changes conclusive only when both capsules record the
same input bytes, problem definition, language, compiler profile and environment,
and measurement harness version. Metric units, kinds, and collection methods
must also match. Missing input identity or a changed comparison context makes
the metric deltas inconclusive; machine-dependent metrics remain inconclusive
across different host fingerprints. Different source code is expected, while a
changed compiler or runtime configuration is shown as a warning because the
observed delta then combines those changes.

`ccx validate` checks generator determinism for every declared size and public
and hidden seed, cross-checks brute force against the reference on the smallest
size across those seeds, and checks every size-matched solution variant against
the reference on all seeds. It measures reference instruction counts for every
size and seed and rejects size medians that fall by more than 10%. Each variant
is also measured three times on one shared input; all Cachegrind counters and
allocation records must agree exactly. Host-dependent memory and timing are not
repeatability gates. Validation uses the selected default compiler and does
not replace a compiler matrix.

`ccx test --cases N --seed S --size NAME` adds the consecutive seed range
`S..S+N-1` at one configured size. The default size is the largest declared
scale. Batches of at most 10,000 cases share one Docker invocation; each
generator, reference answer, submission, and checker still runs as a fresh
process through the pinned `ccx-runner`. The submission uses the exact runtime
argv from its solution configuration, including any fixed environment wrapper.
The `--jobs` option bounds concurrent batches. Inputs and checker files cross process boundaries through sealed
in-memory files, avoiding writable shared files between submission processes.
Each case record includes child wall time, peak RSS, and available Linux
`getrusage` counters: user and system CPU time, minor and major page faults,
context switches, and block-I/O operations. These describe the isolated child;
wall time is an execution diagnostic, not a benchmark score.
`--case-log` writes a JSONL header and every completed case with its seed and
stable case index. Parallel log lines can appear in completion order; consumers
should use `case_index` when sorting or selecting a replay seed. A completed
log ends with a summary record marked `complete: true`; an absent summary means
the command failed before it could write a summary. The generated sweep stops at
the first failing case, while the summary distinguishes requested from
completed cases and marks a skipped or interrupted sweep as incomplete. The
summary retains only a bounded number of individual cases in memory.

`ccx experiment` runs public and hidden seeds across all configured sizes,
then measures only when that suite and any requested generated sweep complete
successfully. Its JSON result contains both reports, and its capsule can be
replayed with `ccx replay`. Large case logs remain separate JSONL files rather
than being embedded in the capsule.

Toolchain images use the Debian snapshot recorded as `apt_snapshot` in
`docker/toolchains.lock.json`, with direct package versions pinned in each
Dockerfile. Toolchain image digests and compiler fingerprints are regenerated
by `scripts/pin-toolchains.sh` after an intentional toolchain change.
CMake 3.31.6 and Ninja 1.12.1 are pinned as build tools in the GCC and Clang
images and recorded in the toolchain lock.

`ccx doctor` runs every pinned compiler in its image and verifies its exact
version and the locked compiler-binary fingerprint, in addition to checking
the harness and build-tool versions.

The toolchain images are currently built locally and are not published to a
registry. Capsules retain the exact local Docker image ID and a stable
environment fingerprint. A replay may use a locally rebuilt image only when
the fingerprint matches the pinned base image, package snapshot, Dockerfile,
compiler and build-tool binaries, source archives, and measurement harness.
`ccx doctor` checks compiler and per-image harness fingerprints against the
lock. The cross-machine workflow compares environment fingerprints but does
not distribute a pullable image.

Each CLI pipeline invocation uses a private host scratch directory so concurrent
build, test, measure, and replay commands cannot replace one another's
intermediate binaries. The directory is removed when the invocation exits.
`ccx build` artifacts that are intended to persist are stored under `.ccx/out`
with their binary SHA-256 in the filename.

The allocation profile runs in a separate Memcheck pass, so its instrumentation
does not change the native timing or Cachegrind instruction counts. The runner
accepts only the pinned `xtree-1` report format and extracts total allocated
blocks, bytes, and freed blocks from the main process report. Threads in that
process are included, but forked and exec'd child profiles are not aggregated.
C, C++, and Rust profiles currently publish these counts. GDC, Zig, Odin, and Go
omit allocator metrics. Allocations that bypass interfaces recognized by
Memcheck are not counted, and xtree does not report peak live allocation bytes.
Go executions use a larger virtual
address-space allowance for the runtime; Docker's memory cgroup remains the
hard physical-memory ceiling, with a 64 MiB allowance for container overhead.
The reported memory verdict still compares the child process peak against the
problem limit.

The container fixes `PATH`, locale, timezone, source date, stack size, CPU set,
network access, process count, memory, and output limits. Source and input
mounts are read-only. The executable runs as a non-root user. Cachegrind numbers
are simulated, not hardware observations. When the sandbox kernel permits
`perf_event_open`, the runner records hardware instructions, CPU cycles, cache
misses, and branch misses alongside the simulated counters. It derives IPC and
an effective average CPU frequency from those events and user CPU time; these
are host-specific summaries, not live frequency samples. A gated child start
excludes counter setup time from the solution's wall-clock interval. Events that
cannot be opened or scheduled are omitted; `ccx doctor` probes the same runner
inside the pinned sandbox image. Hardware events can vary by CPU model and
kernel and are not compared across hosts as if they were simulated counts.

The Linux runner records `getrusage` CPU time, page faults, context switches,
block-I/O operations, and peak RSS for the direct child process. It does not
attribute all activity from detached descendants or report bytes read from
stdin, file-level I/O latency, network latency, or a live CPU-frequency time
series. When hardware cycle events are available, the capsule includes an
effective average frequency computed from cycles and user CPU time. The host
fingerprint records the available frequency policy, not a live frequency
sample. Network access is disabled during submission execution, so a network
latency result requires a deterministic recorded workload and a separate
network benchmark design; no network metric is currently emitted.

Go builds additionally disable module and toolchain downloads, fix `GOROOT`,
cache and temporary paths, and record `GOMAXPROCS=1` with the runtime seed
setting in the capsule environment.

The current [Go capsule](arena/go/capsule.phase1.v0.18.json), ID
`63ae418b8ddae83c5b69c01e0b3f3fb9c30b7c3b73c625c366f6ff450e91d245`, records a
local 20-run measurement at size `good` and seed `2` with harness 0.18.0. It had
0.79% instruction spread, 0.31% branch-miss spread, 1.05% branch spread, 0.67%
memory-reference spread, and 6.69% cache-miss spread. Candidate wall-clock
spread was 56.43%; the paired C++ reference/candidate ratio had 101.39% spread
and a 0.714 median with a 0.085 bootstrap half-width. The three clean builds
had a 14.45-second median wall time and 15.09% spread. These are observations
from one host and one program. The cache-miss and timing spreads are too broad
for ranking Go solutions; instruction spread is also above the configured
0.01% repeatability threshold. The ratio compares Go with a C++ reference and
is cross-language context only. This capsule is evidence about this runtime
and workload, not a cross-machine result. The older `capsule.phase1.json` file
records the 0.16.0 sample and is not replayable by the current harness.

A current GCC C++ measurement at size `naive`, seed `1`, and twenty fresh-process
runs is stored as [capsule.phase1.v0.18.json](arena/naive/capsule.phase1.v0.18.json).
Its matching optimized run is [here](arena/good/capsule.phase1.v0.18.json).
Instructions, Cachegrind cache and branch metrics, Memcheck allocations and
bytes, and binary size had 0.000% within-host spread. Candidate wall-clock
spread was 129.28% for naive and 160.90% for good; paired reference/candidate
ratios had 208.36% and 139.25% spread. These wide ranges are one-host evidence
that wall time remains too noisy for ranking in this environment. The
deterministic metrics repeat exactly here, not as a cross-machine claim.

Ten current harness-0.18.0 capsules cover the supported languages and single- or
multi-file layouts. Their observed instruction, simulated cache-miss, and
candidate wall-clock spreads on this one host are:

| Capsule | Language | Instructions | Cache misses | Wall time |
| --- | --- | ---: | ---: | ---: |
| [C multi-file](arena/c-multifile/capsule.phase1.v0.18.json) | C | 0.00% | 0.00% | 169.44% |
| [D](arena/d/capsule.phase1.v0.18.json) | D | 0.00% | 0.00% | 289.48% |
| [Go](arena/go/capsule.phase1.v0.18.json) | Go | 0.79% | 6.69% | 56.43% |
| [C++ good](arena/good/capsule.phase1.v0.18.json) | C++ | 0.00% | 0.00% | 160.90% |
| [C++ multi-file](arena/multi-file/capsule.phase1.v0.18.json) | C++ | 0.00% | 0.00% | 98.90% |
| [C++ naive](arena/naive/capsule.phase1.v0.18.json) | C++ | 0.00% | 0.00% | 129.28% |
| [Odin](arena/odin/capsule.phase1.v0.18.json) | Odin | 0.00% | 0.00% | 268.93% |
| [Odin multi-file](arena/odin-multifile/capsule.phase1.v0.18.json) | Odin | 0.00% | 0.00% | 523.55% |
| [Rust](arena/rust/capsule.phase1.v0.18.json) | Rust | 0.00% | 0.00% | 192.85% |
| [Zig](arena/zig/capsule.phase1.v0.18.json) | Zig | 0.00% | 0.00% | 182.08% |

The original `capsule.phase1.json` files remain as harness-0.16.0 historical
samples. Current replay requires the capsule's harness version. The refreshed
0.18.0 capsules all replayed with `deterministic_match: true` on the local
host. This establishes local replay only; the three-host CI job has not yet
produced a hosted result. Go's instruction and simulated-cache spreads exceed
the C++ CI's current 0.01% gate and should not inherit that tolerance without
cross-machine measurements.
The cross-machine CI comparison reports each host's observed spread for every
deterministic metric and requires it to stay at or below 0.01% by default. CI
uses this default threshold. It also
requires exact agreement between the three host medians for simulated metrics
and machine-independent measured metrics: Memcheck allocation counts and
bytes, file size, and runner byte counts. Hardware `perf` counters,
`getrusage`, and wall-clock values depend on the host and are excluded from
cross-host equality checks.

Capsules hash the solution tree and the problem tree. Entries under
`source.files` beginning with `problem/` are relative to the problem folder;
all other entries are relative to the solution folder. Transient build and
version-control directories are excluded.

The `cross-machine-repeatability` job measures the same C++ source, problem
size, and seed twenty times on Ubuntu 22.04, 24.04, and 26.04. Its comparison
step requires three distinct hosted runner labels, a recorded host fingerprint
for every runner, a within-host spread no higher than 0.01% for deterministic
metrics, and exact agreement between hosts after
checking source, input, harness, compiler fingerprint, and package snapshot
identity, as well as exact equality of the candidate and reference executable
SHA-256 values and reference compiler argv. Image digests and host fingerprints
may differ. Host-dependent
hardware counters, `getrusage` metrics, and wall-clock readings are reported in
the capsules but excluded from cross-host equality checks. The comparison job
then rebuilds the pinned images and replays all three capsules against its
local image, which checks that environment fingerprints permit real cross-host
replay. This checks independent hosted runner environments, not three
bare-metal CPU models. Until the workflow has completed successfully, the
cross-host gate remains unverified.

The schema-11 golden fixture at
`crates/ccx-core/tests/fixtures/capsule-v11.json` records a local GCC run at
size `good`, seed `42`, over twenty repetitions. Its simulated instruction
count is exactly 18,695,144 in every sample. Wall-clock time ranges from 2.906
to 6.875 ms, with a 3.495 ms median and 113.56% min-to-max spread. Replay
matched all deterministic metrics and kept host-dependent metrics within their
sample intervals on the recording host. As a schema-11 capsule it predates
executable digests, so it cannot prove that the original binary bytes were
identical. This is WSL2 evidence from one host; the wide wall-time range is not
a speed ranking.

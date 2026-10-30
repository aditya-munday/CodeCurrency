# Limitations

This repository is an engine scaffold, not a finished public arena. These are
known gaps in the expanded Phase 1 target:

- Build support includes C and C++ with GCC or Clang, Rust with rustc, D with
  GDC, Zig with Zig, Odin with its pinned monthly release, and Go with its
  pinned release. Assembly is generated compiler output through `ccx asm`, not
  a selectable submission language. No additional language profiles are
  included in this phase.
- The current execution image is Linux x86-64. macOS and Windows targets,
  32-bit x86, ARM, and MSVC need separate locked toolchain images and runners.
  `ccx.toml` records the target and refuses an unsupported image profile.
- GCC 13/14 and Clang 18/19 are pinned for C and C++; the config records and
  validates the exact selected version. `ccx matrix` compares these compiler
  profiles and optimization levels, but cannot select arbitrary releases. The
  custom compiler argv template is restricted to C and C++; it cannot replace
  the pinned compiler or choose an arbitrary build system. Other languages use
  their standard build profile.
- Toolchain images are built locally and are not published to a registry.
  New capsules record both the exact local image ID and a stable fingerprint of
  the pinned build environment. Replay accepts a locally rebuilt image only
  when its base, package snapshot, Dockerfile, tool binaries, archives, and
  harness fingerprint match. Pullable image distribution remains outstanding.
- The hosted native CLI matrix passed on Ubuntu 24.04, Windows Server 2022,
  macOS 15 Intel, and macOS 14 Apple Silicon using Rust 1.85.1. Docker build,
  mount, execution, and replay behavior has been validated locally on x86-64
  Linux under WSL2 and in the hosted Ubuntu container workflow. Cross-target
  `cargo check --workspace --all-targets` also passes with warnings denied for
  Windows x86-64 and macOS Intel and Apple Silicon using Rust 1.97.1, but does
  not replace native hosted tests. Windows and macOS Docker Desktop integration
  runs remain outstanding; Apple Silicon also needs certified amd64 emulation or
  native arm64 images.
- CMake builds are currently supported only for C and C++. A project must
  define an executable target named `ccx_solution`; other target names and
  custom multi-configuration generators are not supported.
- Build artifacts larger than 64 MiB are rejected before transfer from the
  sandbox. Artifact collection uses `O_NOFOLLOW | O_NONBLOCK`, checks that the
  opened path is a regular file, and emits no artifact for a final symlink or
  FIFO. The runner integration test covers both cases.
- The checked-in problem set now covers five categories: sorting, data
  structures, parsing, queues, and memory. Its content breadth remains limited
  until additional validated problems are added.
- `validate` now checks all public and hidden seeds for generator determinism
  at every declared size, brute-force agreement on the smallest size, and each
  size-matched solution variant. It samples reference instruction counts for
  every size and seed and rejects size medians that fall by more than 10%; three
  repeated variant measurements must have stable Cachegrind and allocation records.
  All five checked-in problems passed this validation on the local Linux host.
  This is not a calibrated difficulty study, and `validate` does not run the
  selected compiler matrix.
- `ccx test` can run up to one million consecutive deterministic seeds with a
  complete JSONL case log and bounded parallel workers. Batches of up to 10,000
  seeds share a Docker invocation, while every generator, reference, candidate,
  and checker still runs through the per-process runner. The run stops on its
  first failing case and records requested and completed counts. On the
  documented WSL2 host, a 1,000-seed run with four workers completed in 17.09
  seconds, or 58.5 generated seeds per second including build and nine public
  baseline cases. This is an end-to-end sample, not steady-state throughput; a
  million-case run has not been measured and no time-to-completion guarantee is
  established.
- The GDC profile supports build, test, measurement, assembly, replay, and
  source-filtered GCC optimization remarks, but does not report allocation
  metrics.
- The Zig profile supports build, test, measurement, assembly, and replay, but
  does not report allocation metrics or compiler optimization remarks.
- The Odin profile supports build, test, measurement, assembly, and replay, but
  does not report allocation metrics or compiler optimization remarks. It
  builds the staged package directory so sibling `.odin` files are included.
- The Go profile supports build, test, measurement, assembly, replay, and
  source-filtered inlining and escape-analysis remarks, but does not report
  allocation metrics. Its
  runtime needs extra virtual address space; physical memory is capped by the
  Docker cgroup, which permits 64 MiB for container overhead above the problem
  limit. The current harness-0.18.0 20-run local sample at size `good`, seed
  `2`, showed 0.79% instruction spread, 0.31% branch-miss spread, 1.05% branch
  spread, 0.67% memory-reference spread, 6.69% simulated cache-miss spread,
  56.43% candidate wall-clock spread, and 101.39% paired-ratio spread. Cache
  misses and timing are not reliable Go ranking signals; instruction spread
  also exceeds the 0.01% CI threshold. The paired ratio uses a C++ reference
  and is cross-language context only. Go capsules require at least 20 samples so replay can compare
  medians with a meaningful confidence interval. Go supports `-O0` and `-O2`;
  `-O2` uses the Go compiler's default optimized build. Each clean build starts
  with an empty `GOCACHE`, so it recompiles the standard library. The current
  three-build sample had a 17.45-second median build time and 19.89% spread;
  these cold builds are not typical incremental times or a calibrated
  cross-host estimate.
  A 20-run diagnostic with `run_command = ["env", "GOGC=off", "{binary}",
  "{args}"]` did not stabilize the sample: instruction spread increased to
  0.80%, cache-miss spread fell only to 6.53%, and wall-clock spread increased
  to 135.83%. This predates harness 0.11.0. Go documents `GOGC=off` as
  disabling collection, so it is not a fair default runtime profile
  ([Go GC guide](https://go.dev/doc/gc-guide)).
  Disabling address randomization with `setarch` also fails inside the pinned
  non-root container with `Operation not permitted`; address layout remains a
  possible cache-count influence, but this does not explain instruction spread
  by itself.
- Special checkers run as sandboxed programs with input, expected, and actual
  output paths. Checker authors still need to validate that their checker
  rejects malformed and adversarial output.
- The local repeatability report is available. The latest completed hosted CI
  run measured C, C++, Rust, D, Zig, Odin, and Go twenty times on Ubuntu 22.04,
  24.04, and 26.04. Its strict comparison and cross-host replay passed for C,
  C++, Rust, D, Zig, and Odin; Go remained diagnostic because its deterministic
  counters exceeded the 0.01% within-host threshold. All ten current
  harness-0.18.0 single- and multi-file language capsules replayed with
  deterministic metrics matching on the local WSL2 host. The schema-15 hosted
  run passed raw-sample validation and exact deterministic sample-vector
  comparison for the six strictly gated profiles on Ubuntu 22.04, 24.04, and
  26.04. These are hosted VMs, not three bare-metal CPU models, and do not
  certify every CPU microarchitecture.
- Paired wall-clock samples compare every solution with the problem's C++
  reference. For Rust, D, Zig, Odin, and Go this is cross-language context, not
  a same-language ranking signal. Wall-clock remains indicative and noisy; the
  compiler matrix compares selected pinned profiles on one host and input and
  does not produce a cross-host ranking.
- C, C++, and Rust allocation counts use a separate Valgrind Memcheck xtree
  pass for the main process and its threads. Allocations in forked or exec'd
  child processes are not aggregated, so these metrics are not process-tree
  totals. A complete empty profile is recorded as zero allocations and bytes.
  The collector reports total blocks, bytes, and frees, but not peak
  live allocation bytes; custom allocators that Memcheck does not recognize are
  not counted. Direct file forgery and a delayed forked write are covered by the
  runner integration test.
- Hardware instruction, CPU-cycle, cache-miss, and branch-miss counters are
  collected only when Linux `perf_event_open` works inside the actual sandbox.
  Hosts that deny access use Cachegrind's simulated metrics instead. Hardware
  counter scaling and event availability vary by host, so those values are not
  a cross-machine ranking signal.
- Build wall and CPU time are medians from three byte-identical clean builds,
  with the observed range and bootstrap interval. This small sample remains a
  host-specific diagnostic and is not a ranking signal.
  The build report does not include a live CPU-frequency time series, network
  latency, file-level I/O latency, or bytes read from stdin; build wall time
  excludes Docker startup and artifact transfer. An effective average CPU
  frequency is derived from hardware cycles when the sandbox permits those
  events.
- Assembly output and compiler remarks identify their toolchain and expose when
  remarks are unavailable. GCC-family and Clang-family remarks are filtered to
  solution source paths; Go reports source-local inlining and escape analysis.
  Rust, Zig, and Odin do not currently expose remarks in their profiles, and
  line-level source correlation is not implemented for every compiler.
- `scripts/safety-suite.sh` passed all eight hostile cases on the current
  x86-64 Linux Docker host, including timeout, fork bomb, output cap, memory
  exhaustion, stack exhaustion, host-file read, network, and read-only mount
  tampering checks. This does not verify containment on Windows or macOS hosts;
  those host integrations remain outstanding.
- There is no published release or outside-user installation report yet.

Do not interpret these absences as completed Phase 1 gates. They are visible
here so a scaffold feature cannot be mistaken for a verified capability.

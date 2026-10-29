# Changelog

## Unreleased

- Refresh ten single- and multi-file C, C++, Rust, D, Zig, Odin, and Go
  twenty-run capsules for harness 0.18.0; replay each capsule and record the
  local repeatability evidence and current Go metric spreads.
- Let independent hosted runners share a host-class fingerprint in the
  repeatability gate while still requiring three distinct runner labels.
- Advance the measurement harness to 0.18.0. Treat a structurally valid Memcheck
  xtree profile with `totals: (null)` as a measured zero-allocation result.
- Advance the measurement harness to 0.17.0, reject incomplete Cachegrind
  summaries, and sum all traced-process counters. Make the cache model claim
  precise: fixed I1/D1 misses are reported, while host-detected LL misses are
  excluded.
- Extend `ccx experiment` to include source-mapped assembly artifacts and
  compiler optimization remark status and explanations after correctness and
  measurement succeed.
- Repeat candidate clean builds three times, require identical executable
  hashes, and record build wall and CPU time distributions in schema-14 capsules;
  replay repeats the recorded build count. Legacy build-time spreads without a
  recorded build count migrate as single observations.
- Advance the profile catalog to schema version 12 for repeated build timing
  metrics and their uncertainty.
- Require every repeatability metric's sample count to match the run count and
  reject capsules requesting more than 20 candidate builds.
- Bump the runner to harness 0.15.0. Reject malformed resource limits, fail
  closed if a process limit cannot be installed, and report per-stream output
  truncation instead of silently accepting incomplete output.
- Bump the runner to harness 0.16.0. Grow output capture within the combined
  stdout and stderr cap, preserving single-stream output below the configured
  total limit.
- Regenerate and replay all ten checked-in single-file and multi-file language
  capsules against the harness 0.16.0 pinned toolchains.
- Expand all ten checked-in language capsules to twenty fresh-process samples
  and record per-language within-host instruction, cache, and wall-time spreads.
- Expand `ccx validate` to exercise all public and hidden seeds across declared
  sizes, cross-check brute force, validate each matched-size variant, and base
  size separation on median reference instruction counts.
- Run the real hostile-submission suite on the local Linux Docker host; all
  eight containment cases passed. Record the host scope and unverified
  Windows/macOS integrations in the limitations.
- Regenerate and replay the checked-in Phase 1 capsule set under harness 0.14.0.
- Measure one built executable across repeated runs; record build timing as a
  single measured sample and replay by rebuilding once, then bump the harness
  version to 0.11.0.
- Regenerate the naive, optimized, and Go sample capsules under harness 0.11.0;
  record same-binary repeatability and verify deterministic replay.
- Refresh and replay the C, C++, Rust, D, Zig, Odin, and Go single- and
  multi-file capsule examples under harness 0.11.0.
- Advance the profile catalog to schema version 10 for the measured,
  host-dependent single-observation build timing metric.
- Pair each solution timing with an alternating reference timing; store the
  reference duration and per-pair speed ratio with repeatability intervals.
- Advance the profile catalog to schema version 11 for paired reference metrics
  and bump the measurement harness to 0.12.0.
- Advance experiment capsules to schema version 13 with reference executable
  digests and compiler argv; replay and cross-host comparison verify both sides
  of paired timing.
- Migrate schema-12 capsules without inventing reference provenance, then
  regenerate and replay the checked-in Phase 1 capsule set under harness 0.13.0.
- Fixed Cachegrind's simulated cache geometry across hosts and bumped the
  measurement harness version to 0.9.0.
- Replace in-process allocator wrapping with an isolated Memcheck xtree pass for
  C, C++, and Rust; reject malformed or submission-forged reports, include
  allocation metrics in cross-machine checks, and bump the harness to 0.10.0.
- Make host metadata detection compile cleanly across Windows and macOS targets;
  enforce Rust warnings-as-errors in the CI matrix.

- Add a checked multi-file C++ sample with header, separate implementation,
  direct and CMake builds, hidden-case validation, and capsule replay coverage.
- Build Odin projects as package directories so sibling source files are
  compiled into the solution and emitted assembly.
- Add C multi-file direct and CMake examples with test, replay, and generator
  matrix coverage; split the Rust, D, and Zig samples across source modules.
- Add a Go multi-file sample and run its hidden suite through the pinned Go
  builder.
- Bound compiled-artifact transfer to 64 MiB, accept only regular artifact files, and test the exact size boundary, oversized files, symlinks, and FIFOs.
- Refresh the checked-in C++ comparison capsules against the current schema-12 toolchain environment and update their replay IDs and observed timing intervals.
- Refresh checked-in Rust, D, Zig, Odin, and Go capsules against the current schema-12 toolchain environment and verify deterministic replay for each language.
- Record the Go sample's harness-0.9.0 20-run instruction, branch, memory-reference, cache, and wall-clock spreads from the pinned runtime.
- Advance experiment capsules to schema version 12 with the measured executable SHA-256; reject replay when rebuilt bytes differ and require matching binary digests in the cross-machine gate.
- Advance the profile catalog to schema version 9 and list language coverage for each metric, including allocator metrics not instrumented by every runtime.
- Add `ccx experiment` to run public and hidden correctness tests before measurement, returning one combined test and capsule JSON report.
- Advance experiment capsules to schema version 11 with observed per-metric p95 and p99 sample quantiles; migrate older capsules without fabricating tail data.
- Include host, execution target, pinned toolchain environment, observed spread, and explicit uncertainty kinds in measurement JSON; preserve fractional ratios and IPC in tables.
- Advance capsules to schema version 10 with stable toolchain environment fingerprints, allowing replay across locally rebuilt images only when their pinned recipe and tool binaries match.
- Verify the measurement harness binary fingerprint separately in each GCC, Clang, and Rust image; expose stable environment fingerprints in the profile catalog schema version 8.
- Replay each of the three hosted repeatability capsules on the comparison runner after validating their shared environment fingerprint.
- Extend `ccx matrix` to compare direct, CMake with Ninja or Make, and bounded custom C/C++ builds on identical generated input.
- Make successful measurements fail clearly when Cachegrind cannot produce its required simulated counters; follow configured runtime wrappers while measuring.
- Apply the configured runtime argv to every batched correctness case, bump the harness version, and repin all execution images.
- Expose MSVC as an explicitly unavailable C/C++ profile until a pinned Windows runner is available.
- Advance the profile catalog to schema version 6 with explicit x86, x86-64, and ARM64 choices for Linux, Windows, and macOS.
- Advance capsules to schema version 9 and record runner-measured stdout and stderr byte totals.
- Expand Cachegrind reporting to separate instruction and data cache misses, reads, writes, branch instructions, and simulated miss rates.
- Bump the measurement harness version for the expanded result metrics.
- Advance the profile catalog to schema version 5 with exact IDs for the expanded metrics and custom command capabilities.
- Add custom C/C++ compiler argv templates and sandboxed runtime argv templates, both recorded and checked by replay.
- Add pinned GCC 13 and Clang 18 profiles alongside GCC 14 and Clang 19, selectable by configuration and comparable through the compiler matrix.
- Advance experiment capsules to schema version 8 with a version 7 migration for versioned compiler identities.
- Verify each compiler's exact version and binary fingerprint inside its pinned image with `ccx doctor`.
- Derive compiler, language, build-system, and Linux target availability from pinned toolchain and build-tool lock entries; advance the profile catalog to schema version 3 with per-choice availability and reasons.
- Let C and C++ projects choose the pinned CMake generator, Ninja or Unix Makefiles, and include the choice in configuration, build recipes, profile discovery, and `doctor` checks.
- Expose explicit OS and architecture selectors in `ccx init` and reject targets without a pinned execution profile.
- Limit selectable submission languages to C, C++, Rust, D, Zig, Odin, and Go while retaining generated assembly output through `ccx asm`.
- Report only executed cases in `ccx test` totals when a failing public case skips the requested generated sweep.
- Record the observed 1,000-seed generated-test throughput sample and its end-to-end limits.
- Add a versioned `ccx profiles` JSON catalog for language, compiler, build, target, and metric choices, including explicit unavailable profiles.
- Batch generated correctness cases inside pinned containers while retaining fresh per-case runner processes and sealed read-only input buffers.
- Kill leftover process-group descendants after each runner invocation, including programs that close stdout and stderr before hanging.
- Stop generated sweeps at the first failing case and report requested versus completed counts.
- Add `ccx matrix` to measure selected pinned compiler and optimization profiles into separate replayable capsules, verify shared input hashes, preserve results by capsule identity, and label pairwise differences within repeatability noise.
- Allow replay to locate a solution above a nested capsule path or accept an explicit `--dir` for externally stored capsules.
- Add per-case Linux process resource counters to correctness reports and JSONL logs.
- Isolate concurrent CLI pipeline invocations in private scratch directories and store successful build artifacts by binary hash.
- Build x86-64 assembly as assembly, omit unsupported optimization claims, and keep its generated source view per invocation.
- Add measured Linux process CPU time, page faults, context switches, and block-I/O operation counts to capsules.
- Add repeated clean-build wall and CPU timing to capsules and reject byte-different repeated builds.
- Add bounded parallel seeded sweeps of up to one million cases with streaming JSONL logs.
- Add explicit compiler version, target profile, standard/custom preset, and bounded C/C++ compiler flags to `ccx.toml` and `ccx init`.
- Refresh pinned image IDs and the harness hash for the resource-usage runner.
- Bump the harness version for repeated build measurements.
- Collect optional Linux hardware instructions, cache misses, branch misses, and CPU cycles with `perf_event_open`; derive host-specific IPC and effective average frequency, and fall back to simulated Cachegrind counts when unavailable.
- Make `ccx doctor` probe hardware counters inside the actual restricted container and report the active measurement methods.
- Bump the harness version and refresh all toolchain image IDs and harness hashes for hardware counter support.
- Allow bounded runtime argv customization through `ccx init` and `ccx.toml`, include it in experiment capsules, and reject replay when the configured arguments differ.
- Stage multi-file projects into isolated read-only build inputs so nested includes and private solution-directory permissions work with the non-root sandbox.
- Advance the profile catalog to schema version 2 for its runtime-argument capability.
- Add pinned Go support for build, test, measurement, disassembly, and replay.
- Advance experiment capsules to schema version 7 with Go migration.
- Bump the harness version for interval-based replay and repeated replay samples.

- Add pinned Zig 0.17.0 and Odin dev-2026-10 build, test, measurement,
  assembly, and replay support.
- Add Zig and Odin sorted pair sum solutions to the language validation matrix.
- Advance experiment capsules to schema version 6 with Zig and Odin migrations.

- Added D builds using pinned GDC 14.2.0 and schema version 4.
- Added a concurrent allocator instrumentation regression suite.

- Added repeated Cachegrind samples to `ccx measure` and median-based reported
  metric values.
- Added indicative wall-clock samples and seeded confidence intervals to
  capsules.
- Added recursive project input hashing and nested source discovery.
- Added explicit GCC or Clang selection to `ccx init`.
- Added initial trust methodology, authoring, quickstart, and limitations docs.

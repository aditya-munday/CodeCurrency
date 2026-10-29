# CodeCurrency

CodeCurrency is a local performance arena for systems code. A result records
the exact source tree, toolchain image, input seed, machine, and measurement
methods so another person can inspect it and replay it when a local image with
the same pinned environment fingerprint is available.

The current engine supports C and C++ with pinned GCC and Clang images, Rust
with a pinned `rustc` image, D with pinned GDC, Zig with a pinned Zig compiler,
Odin with a pinned monthly release, and Go with a pinned release. It runs seeded
problems in an isolated Docker container and emits JSON experiment capsules.
Assembly is available as generated compiler output through `ccx asm`, not as a
selectable submission language.

## Quick start

Requirements: Rust 1.85 or later and Docker with Linux-container support. The
CI workflow is configured to build and test the CLI on Linux, Windows, and
macOS, but no hosted workflow run is recorded yet. Local end-to-end Docker
measurements are verified on x86-64 Linux. See
[platform support](PLATFORM_SUPPORT.md) for host/compiler options and their
validation status.
Debian package versions and the archive snapshot are pinned in the Dockerfiles
and `docker/toolchains.lock.json`.
The first image build needs access to the pinned Debian snapshot unless its
layers are already cached. Submission builds and runs use network-disabled
containers.

```sh
scripts/pin-toolchains.sh
cargo build --release
target/release/ccx doctor
target/release/ccx init --problem sorted-pair-sum --language cpp --toolchain gcc --dir arena/my-run
```

For a C or C++ CMake scaffold, pass `--build-system cmake`. Ninja is the
default generator; pass `--cmake-generator make` to use pinned Unix Makefiles.
Both generators use tools pinned in the toolchain lock.

The default `standard` preset writes the pinned compiler version, Linux target,
and x86-64 architecture into `ccx.toml`. C and C++ can select GCC 13 or 14 and
Clang 18 or 19 from the pinned toolchain images, then select the bounded
`custom` preset for allow-listed code-generation flags.

`ccx init` also accepts `--target-os` and `--target-arch`. The current verified
execution target is `linux/x86_64`; other requested targets fail with a profile
availability message instead of producing a scaffold that cannot run.

```sh
target/release/ccx init --problem sorted-pair-sum --language cpp --toolchain clang \
  --compiler-version 19.1.7 --build-preset custom --compiler-flag=-mavx2 \
  --compiler-flag=-funroll-loops --dir arena/custom-run
```

For a custom C/C++ compiler argv template, select `--build-system custom`. It
uses the pinned compiler selected by `--toolchain`; `build_command` in
`ccx.toml` can put `{flags}` and `{sources}` in either order while `{output}`
always names the isolated artifact path. `compiler_flags` carries the
allow-listed code-generation options. This keeps the compiler, source set, and
artifact path under engine control while supporting custom compiler layouts.

Runtime options can be set with repeated `--program-arg` values or as the
`program_args` array in `ccx.toml`. An optional `run_command` array can wrap the
executable, for example `[
"env", "MODE=fast", "{binary}", "--mode", "{args}"
]`. Its tokens are passed as argv without shell parsing. The expanded command
is stored in the capsule and checked by replay. Custom runtime commands still
run under the same time, memory, process, output, filesystem, and network
limits. Plugin loading, response files, host paths, output controls, and
host-native CPU tuning are rejected from compiler flags. Compiler versions
must match an image pinned in the toolchain lock.
`ccx profiles --format json` lists the available and unavailable language,
compiler, build, and execution-target choices in a versioned catalog that an
editor or other client can use without guessing. It also labels collected and
unavailable metrics. Currently the execution target is Linux x86-64; other OS
and architecture choices remain disabled until their runner and toolchain are
pinned.

Solution folders can contain multiple source files, headers, and nested source
directories. The engine snapshots those files into a read-only staging tree for
each pipeline run, preserving relative include paths while keeping builds
independent from later editor changes.

`arena/multi-file` is a working C++ example with a separate header and
implementation file. The Linux container CI workflow builds it and runs all
declared public and hidden cases, then measures CMake with Ninja and Make,
alongside the single-file quickstart.

`arena/odin-multifile` demonstrates an Odin package split across sibling source
files; the Odin backend builds the complete staged package directory.

`arena/c-multifile` exercises C headers and implementation files through both
direct GCC and CMake builds.

The Rust, D, Zig, and Go sorted pair sum samples also split counting logic into
separate source files; CI runs their public and hidden suites.

Edit `arena/my-run/main.cpp`, then run:

```sh
target/release/ccx test --dir arena/my-run --hidden
target/release/ccx measure --dir arena/my-run --size good --seed 42 --repeat 20
target/release/ccx matrix --dir arena/my-run --opt-level -O2 --opt-level -O3 \
  --size good --seed 42 --repeat 20
target/release/ccx matrix --dir arena/my-run --toolchain gcc --toolchain clang \
  --build-system direct --build-system custom \
  --size good --seed 42 --repeat 20
target/release/ccx replay arena/my-run/capsule.json
target/release/ccx asm --dir arena/my-run
```

`ccx asm` returns source-focused compiler explanations where the selected
profile provides them. Its JSON report includes `remarks_status`,
`remarks_method`, and `remarks_explanation`, so an unsupported diagnostic pass
is distinguishable from an optimizer that emitted no remarks.

For the full one-command path, `experiment` runs public and hidden seeds at
every configured size, optionally adds a generated stress sweep, then measures
and explains the selected input only if correctness passes. Its combined report
includes the capsule metrics and hints, source-mapped assembly artifacts, and
the compiler remarks status and explanation. Toolchains without stable
optimization diagnostics report that limitation explicitly. The capsule can be
replayed separately.

```sh
target/release/ccx experiment --dir arena/my-run --size good --seed 42 \
  --repeat 20 --cases 1000 --jobs 4 --case-log arena/my-run/cases.jsonl
target/release/ccx replay arena/my-run/capsule.json
```

For a larger deterministic seed sweep, add `--cases`, choose a size, and retain
the complete per-case record as JSON Lines:

```sh
target/release/ccx test --dir arena/my-run --hidden --cases 1000000 \
  --seed 900000000 --size great --jobs 4 --case-log stress.jsonl
```

Generated seeds are consecutive and replayable from the reported seed and
size. `--jobs` runs isolated case batches concurrently. Each batch reuses one
container for up to 10,000 cases, while the generator, reference, candidate,
and checker still run as fresh processes under the pinned runner and per-process
limits. Case inputs and checker files are passed through sealed in-memory files;
the submission cannot change them or access the host. A case log contains a
stable case index because parallel batches can finish in a different order.
The JSON summary keeps only a bounded number of individual results in memory;
the JSONL file retains every case and ends with a `complete: true` summary
record when the full sweep finished. Generated sweeps stop at the first failing
case; the summary reports requested and completed counts and marks an early
stop as incomplete. A failing public seed skips the generated sweep.

`ccx measure` performs three clean builds, requires identical executable
hashes, then runs that executable with the same input in a fresh process for
every repetition. Each candidate wall-clock sample is
paired with a fresh C++ reference run on the same input, with execution order
alternating between samples. The capsule records both times and the per-pair
reference/candidate ratio; values above one mean the candidate was faster for
that pair. These times are indicative, and ratios for non-C++ submissions are
cross-language context rather than a same-language ranking signal. Simulated
Cachegrind counters and native process measurements are collected for each
candidate run; reported values are medians with a seeded bootstrap interval.
Build wall time and CPU time are medians of three measured, host-dependent
clean builds, with the observed spread and bootstrap interval in the capsule.
Execution wall time and peak
memory are never ranking signals.
Linux `getrusage` also records process CPU time, page faults, context switches,
and block-I/O operation counts; these are measured OS counters and are not
network or file-latency measurements. When permitted by the host kernel, the
runner additionally records hardware CPU cycles and derives IPC and effective
average CPU frequency; these values are machine-specific and omitted when the
hardware events are unavailable.

`ccx matrix` measures compiler, optimization, build-system, and CMake generator
profiles on the same problem, size, and seed, then writes one replayable capsule
per profile. C and C++ default to the pinned GCC and Clang profiles; other
languages default to their configured compiler. Select configurations with
repeated `--toolchain`, `--opt-level`, `--build-system`, and
`--cmake-generator` options. The current lock pins specific compiler versions;
it does not accept arbitrary releases. Capsules saved outside the solution
directory can be replayed with `ccx replay <capsule> --dir
<solution-directory>`. The combined report marks ranked-metric differences
within the combined repeatability interval as `within_noise`; this is a
same-host comparison aid, not a ranking. It records the shared input hash and
refuses to compare profiles if generated input bytes differ. Capsule names
include the profile and content ID, preserving earlier matrix results when you
run another experiment. CMake profiles need a project `CMakeLists.txt`; custom
profiles are limited to the safe C/C++ argv template.

## Commands

`ccx doctor`, `init`, `build`, `run`, `test`, `measure`, `matrix`, `asm`, `diff`,
`replay`, and `validate` are available. Add `--format json` for machine-readable
reports. `ccx build` stores the binary under `.ccx/out` using its SHA-256 in the
filename; intermediate files use a per-invocation scratch directory. Run
`ccx --help` and `ccx <command> --help` for arguments.

## Problems

Each problem lives in `problems/<id>/` and includes metadata, a statement,
seeded C generator, brute-force oracle, reference C++ solution, editorial,
hints, and three C++ performance variants. `ccx validate` checks the oracle,
variants, and three-run stability of their counters and allocation records. See
[problem authoring](PROBLEM_AUTHORING.md).

The included problems cover pair counting, top-K selection, delimited parsing,
FIFO queues, and record storage layouts.

See [a measured pair-counting comparison](PERFORMANCE_FINDINGS.md) for a
replayable example with source-mapped assembly, counters, and capsule diffs.

## Trust and limitations

Read [METHODOLOGY.md](METHODOLOGY.md), [CAPSULE_SPEC.md](CAPSULE_SPEC.md), and
[LIMITATIONS.md](LIMITATIONS.md) before comparing or citing results. Simulated
counter values are comparable only when the measured binary, input, toolchain,
and simulator configuration match. Wall-clock time is host-specific.

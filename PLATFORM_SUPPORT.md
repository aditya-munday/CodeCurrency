# Platform and compiler support

## Host operating systems

| Host | Native CLI and unit tests | Container builds and measurements | Current validation |
| --- | --- | --- | --- |
| Ubuntu 22.04 or 24.04, x86-64 | Hosted CLI tests passed on Ubuntu 24.04 | Linux Docker workflow exercised on Ubuntu 24.04 | 20-run measurement and capsule replay passed on Ubuntu 22.04 and 24.04 |
| Ubuntu 26.04 under WSL 2, x86-64 | Locally exercised | Verified with Docker Desktop Linux containers | Local build, test, experiment, replay, and CMake paths exercised; hosted repeatability passed |
| Windows 11, x86-64 | Hosted CLI tests passed on Windows Server 2022 | Docker Desktop Linux-container integration remains unverified | Native build, test, and formatting checks passed in CI |
| macOS 15 Intel, x86-64 | Hosted CLI tests passed on macOS 15 Intel | Docker Desktop integration remains unverified | Native build, test, and formatting checks passed in CI |
| macOS 14 Apple Silicon, arm64 | Hosted CLI tests passed on macOS 14 | Pinned language images are x86-64; amd64 emulation is not certified | Native build, test, and formatting checks passed in CI |

The container is the execution environment on every host. Windows submissions
use Linux containers, not Windows containers. Keep Windows checkouts inside the
WSL 2 filesystem when using Docker Desktop. Linux source trees must be available
to the selected Docker context as read-only bind mounts. Container execution
has only been end-to-end verified on x86-64 Linux so far. Hosted native CLI
checks pass on Windows and macOS, but their Docker Desktop integration remains
unverified.

From the Linux development host, `cargo check --workspace --all-targets` with
`RUSTFLAGS=-D warnings` passed for `x86_64-pc-windows-gnu`,
`x86_64-apple-darwin`, and `aarch64-apple-darwin` using the installed Rust
1.97.1 toolchain. These checks compile each target's code and tests but do not
verify native linking, test execution, or Docker integration. The hosted matrix
still uses the pinned Rust 1.85.1 toolchain and remains the native authority.

## Pinned tools

The authoritative image digests and compiler fingerprints are in
`docker/toolchains.lock.json`. `scripts/pin-toolchains.sh` rebuilds the images
from a fixed Debian 13 base digest and the recorded Debian package snapshot.

| Profile | Version in the pinned image | Compiler selection in `ccx.toml` |
| --- | --- | --- |
| C | GCC 13.3.0 or 14.2.0; Clang 18.1.8 or 19.1.7 | `gcc-13`, `gcc`, `clang-18`, or `clang` |
| C++ | GCC 13.3.0 or 14.2.0; Clang 18.1.8 or 19.1.7 | `gcc-13`, `gcc`, `clang-18`, or `clang` |
| Rust | rustc 1.85.1 | `toolchain = "rust"` |
| D | GDC 14.2.0 | `toolchain = "gdc"` |
| Zig | Zig 0.17.0 | `toolchain = "zig"` |
| Odin | `dev-2026-10-nightly:84bc3fc` | `toolchain = "odin"` |
| Go | Go 1.26.10 | `toolchain = "go"` |

The CLI workspace declares Rust 1.85 as its minimum version. The hosted native
CI matrix passed with Rust 1.85.1 on Ubuntu 24.04, Windows Server 2022, macOS 15
Intel, and macOS 14 Apple Silicon. The C, C++, Rust, D, Zig, Odin, and Go
submission builds run inside digest-pinned Linux images, so the host compiler
installation is not used to build a measured solution. `ccx asm` emits assembly
for these compiled submissions; assembly is not a selectable language profile.

Pinned CMake projects use CMake 3.31.6 and can select Ninja 1.12.1 or GNU Make
4.4.1 from the same Debian snapshot. Versions and binary hashes are recorded
under `build_tools` in `docker/toolchains.lock.json` and checked by `ccx doctor`.

C and C++ solutions can use direct compiler invocation, CMake with Ninja or
Unix Makefiles, or a custom argv template using the selected pinned compiler.
`ccx init --build-system cmake` creates a minimal CMake project with the
`ccx_solution` target by default. Pass `--cmake-target <name>` to scaffold a
different executable target, or set `cmake_target` in `ccx.toml` to match an
existing project. `--cmake-generator make` selects GNU Make. CMake runs with
network access disabled and its build tree lives in the container's temporary
scratch filesystem. The capsule records the exact configure and build recipe
along with the CMakeLists and `ccx.toml` source hashes.

`ccx matrix` runs the selected pinned compiler profiles and optimization levels
against one shared problem, size, and seed, producing a capsule for every
profile. C and C++ default to every pinned GCC and Clang version; other
languages use their configured pinned compiler. Each compiler identity is a
separate locked profile, and selecting an unpinned release remains unavailable.

## Solution configuration

`ccx.toml` records the selected language, toolchain, exact pinned compiler
version, build system, preset, target OS, and target architecture. The standard
preset uses the toolchain defaults. The bounded custom preset currently accepts
allow-listed C/C++ code-generation flags; it rejects plugins, response files,
host paths, output controls, shell syntax, and host-native CPU tuning. CMake
include paths belong in `CMakeLists.txt`. `build_system = "custom"` selects a
fixed argv template with placeholders for the pinned compiler, validated flags,
source list, and output path. It does not run a shell or replace the compiler.
An optional `run_command` argv template expands `{binary}` and `{args}` and is
executed by the same restricted runner; its expanded command is saved in the
capsule. Template values are passed directly to `exec`, without shell parsing.

The only end-to-end execution target currently pinned is `linux/x86_64`. The
toolchain lock has two versions each for GCC and Clang and one version each for
rustc, GDC, Zig, Odin, and Go. MSVC, further compiler releases, macOS and Windows execution images,
32-bit x86, and ARM targets need separate pinned images and runner support;
changing the target in `ccx.toml` is rejected until that profile exists.
`ccx profiles --format json` exposes these available and unavailable choices as
a versioned catalog for editor and client configuration screens. The C and C++
catalogs include an unavailable `msvc` choice with the missing Windows backend
listed as its reason.

The target catalog names Linux, Windows, and macOS profiles for x86, x86-64,
and ARM64. Only Linux x86-64 is enabled. A platform name in the catalog means
the configuration UI can explain and disable that choice; it does not claim
that a matching compiler image or execution sandbox exists.

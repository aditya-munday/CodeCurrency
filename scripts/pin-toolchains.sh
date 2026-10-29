#!/usr/bin/env bash
# Build the pinned toolchain images and write docker/toolchains.lock.json.
#
# Layer 2 requires digests, never tags. This script builds each image, reads
# back the content digest, and records it along with the compiler version and
# libc version actually present inside the image.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

BASE_DIGEST="sha256:62bd423aeee912639ca8ee681c2b762cb3e25137e0a92b20be455898a9f034ad"
DEBIAN_SNAPSHOT="20261020T213255Z"
GCC_TAG="ccx-gcc:pin"
CLANG_TAG="ccx-clang:pin"
RUST_TAG="ccx-rust:pin"
ZIG_ARCHIVE="docker/zig-x86_64-linux-0.17.0.tar.xz"
ZIG_CACHE="/tmp/ccx-zig-x86_64-linux-0.17.0.tar.xz"
ZIG_SHA256="1cbe9df9f27e6b78d14ccbca43b6703a404ef79ef1c463de901d7f088d4e2026"
ODIN_ARCHIVE="docker/odin-linux-amd64-dev-2026-10.tar.gz"
ODIN_CACHE="/tmp/ccx-odin-linux-amd64-dev-2026-10.tar.gz"
ODIN_SHA256="c3c8b095621fd0c75f7f73e3a0829f1b4d45324225f20ba11ed8dc4da310a8ab"
GO_ARCHIVE="docker/go1.26.10.linux-amd64.tar.gz"
GO_CACHE="/tmp/ccx-go1.26.10.linux-amd64.tar.gz"
GO_SHA256="ccfe7ae822d2c2f41f4fe7c956199967a4dac22d0ee0d2b2bfdb440aab3b20c5"

prepare_archive() {
  local archive="$1"
  local cache="$2"
  local expected="$3"
  local url="$4"

  if echo "$expected  $cache" | sha256sum --check --status 2>/dev/null; then
    return
  fi
  rm -f "$cache"
  if [[ -f "$repo_root/$archive" ]]; then
    cp "$repo_root/$archive" "$cache"
  else
    curl --fail --location --silent --show-error --retry 12 --retry-all-errors --retry-delay 2 \
      --continue-at - "$url" -o "$cache"
  fi
  echo "$expected  $cache" | sha256sum --check
}

prepare_archive "$ZIG_ARCHIVE" "$ZIG_CACHE" "$ZIG_SHA256" \
  https://ziglang.org/download/0.17.0/zig-x86_64-linux-0.17.0.tar.xz
prepare_archive "$ODIN_ARCHIVE" "$ODIN_CACHE" "$ODIN_SHA256" \
  https://github.com/odin-lang/Odin/releases/download/dev-2026-10/odin-linux-amd64-dev-2026-10.tar.gz
prepare_archive "$GO_ARCHIVE" "$GO_CACHE" "$GO_SHA256" \
  https://go.dev/dl/go1.26.10.linux-amd64.tar.gz

TOOLCHAIN_BUILD_CONTEXT="$(mktemp -d "${TMPDIR:-/tmp}/ccx-toolchain-images.XXXXXX")"
trap 'rm -rf "$TOOLCHAIN_BUILD_CONTEXT"' EXIT
cp -a docker/. "$TOOLCHAIN_BUILD_CONTEXT/"
cp "$ZIG_CACHE" "$TOOLCHAIN_BUILD_CONTEXT/$(basename "$ZIG_ARCHIVE")"
cp "$ODIN_CACHE" "$TOOLCHAIN_BUILD_CONTEXT/$(basename "$ODIN_ARCHIVE")"
cp "$GO_CACHE" "$TOOLCHAIN_BUILD_CONTEXT/$(basename "$GO_ARCHIVE")"

echo "==> building $GCC_TAG"
docker build -q --build-arg "DEBIAN_SNAPSHOT=$DEBIAN_SNAPSHOT" -f "$TOOLCHAIN_BUILD_CONTEXT/gcc.Dockerfile" -t "$GCC_TAG" "$TOOLCHAIN_BUILD_CONTEXT" >/dev/null

echo "==> building $CLANG_TAG"
docker build -q --build-arg "DEBIAN_SNAPSHOT=$DEBIAN_SNAPSHOT" -f "$TOOLCHAIN_BUILD_CONTEXT/clang.Dockerfile" -t "$CLANG_TAG" "$TOOLCHAIN_BUILD_CONTEXT" >/dev/null

echo "==> building $RUST_TAG"
docker build -q --build-arg "DEBIAN_SNAPSHOT=$DEBIAN_SNAPSHOT" -f "$TOOLCHAIN_BUILD_CONTEXT/rust.Dockerfile" -t "$RUST_TAG" "$TOOLCHAIN_BUILD_CONTEXT" >/dev/null

gcc_digest=$(docker inspect --format='{{index .RepoDigests 0}}' "$GCC_TAG" 2>/dev/null || true)
clang_digest=$(docker inspect --format='{{index .RepoDigests 0}}' "$CLANG_TAG" 2>/dev/null || true)
rust_digest=$(docker inspect --format='{{index .RepoDigests 0}}' "$RUST_TAG" 2>/dev/null || true)

# Locally built images have no RepoDigests until pushed; fall back to the
# image ID, which is itself a content digest and is what `docker run` resolves.
if [[ -z "$gcc_digest" ]]; then
  gcc_digest="ccx-gcc@$(docker inspect --format='{{.Id}}' "$GCC_TAG")"
fi
if [[ -z "$clang_digest" ]]; then
  clang_digest="ccx-clang@$(docker inspect --format='{{.Id}}' "$CLANG_TAG")"
fi
if [[ -z "$rust_digest" ]]; then
  rust_digest="ccx-rust@$(docker inspect --format='{{.Id}}' "$RUST_TAG")"
fi

gcc_version=$(docker run --rm --entrypoint gcc "$GCC_TAG" -dumpfullversion -dumpversion | head -1)
gcc13_version=$(docker run --rm --entrypoint gcc-13 "$GCC_TAG" -dumpfullversion -dumpversion | head -1)
# `clang --version` prints a long banner; keep just the numeric version.
clang_version=$(docker run --rm --entrypoint clang "$CLANG_TAG" -dumpversion | head -1)
clang18_version=$(docker run --rm --entrypoint clang-18 "$CLANG_TAG" -dumpversion | head -1)
rust_version=$(docker run --rm --entrypoint rustc "$RUST_TAG" --version | awk '{print $2}')
gdc_version=$(docker run --rm --entrypoint gdc-14 "$GCC_TAG" -dumpfullversion -dumpversion | head -1)
zig_version=$(docker run --rm --entrypoint zig "$GCC_TAG" version | head -1)
odin_version=$(docker run --rm --entrypoint odin "$CLANG_TAG" version | head -1 | tr -d '\r')
go_version=$(docker run --rm --entrypoint go "$GCC_TAG" version | awk '{print $3}')
cmake_version=$(docker run --rm --entrypoint cmake "$GCC_TAG" --version | awk 'NR == 1 {print $3}')
ninja_version=$(docker run --rm --entrypoint ninja "$GCC_TAG" --version | head -1)
make_version=$(docker run --rm --entrypoint make "$GCC_TAG" --version | awk 'NR == 1 {print $3}')
# `ldd --version` prints "ldd (Debian GLIBC 2.41-12+deb13u4) 2.41"; keep "glibc 2.41".
libc_version=$(docker run --rm --entrypoint ldd "$GCC_TAG" --version | head -1 | sed -E 's/.*GLIBC ([0-9.]+).*/glibc \1/')

# A locally built image ID is a content digest, but it is not stable across
# rebuilds because apt layer timestamps differ. The compiler binaries are,
# though, so hash them: this is the machine-independent identity of the
# toolchain, and it is what a cross-machine determinism check compares.
gcc_sha=$(docker run --rm --entrypoint sha256sum "$GCC_TAG" /usr/bin/gcc /usr/bin/g++ | sha256sum | cut -d' ' -f1)
gcc13_sha=$(docker run --rm --entrypoint sha256sum "$GCC_TAG" /usr/bin/gcc-13 /usr/bin/g++-13 | sha256sum | cut -d' ' -f1)
clang_sha=$(docker run --rm --entrypoint sha256sum "$CLANG_TAG" /usr/bin/clang | sha256sum | cut -d' ' -f1)
clang18_sha=$(docker run --rm --entrypoint sha256sum "$CLANG_TAG" /usr/bin/clang-18 | sha256sum | cut -d' ' -f1)
rust_sha=$(docker run --rm --entrypoint sha256sum "$RUST_TAG" /usr/bin/rustc | sha256sum | cut -d' ' -f1)
gdc_sha=$(docker run --rm --entrypoint sha256sum "$GCC_TAG" /usr/bin/gdc-14 | sha256sum | cut -d' ' -f1)
zig_sha=$(docker run --rm --entrypoint sha256sum "$GCC_TAG" /opt/zig/zig | sha256sum | cut -d' ' -f1)
odin_sha=$(docker run --rm --entrypoint sha256sum "$CLANG_TAG" /opt/odin/odin | sha256sum | cut -d' ' -f1)
go_sha=$(docker run --rm --entrypoint sha256sum "$GCC_TAG" /usr/local/go/bin/go | sha256sum | cut -d' ' -f1)
cmake_sha=$(docker run --rm --entrypoint sha256sum "$GCC_TAG" /usr/bin/cmake | awk '{print $1}')
ninja_sha=$(docker run --rm --entrypoint sha256sum "$GCC_TAG" /usr/bin/ninja | awk '{print $1}')
make_sha=$(docker run --rm --entrypoint sha256sum "$GCC_TAG" /usr/bin/make | awk '{print $1}')
harness_sha=$(docker run --rm --entrypoint sha256sum "$GCC_TAG" /opt/ccx/runner /opt/ccx/batch-runner /opt/ccx/alloc_shim.o | sha256sum | cut -d' ' -f1)
clang_harness_sha=$(docker run --rm --entrypoint sha256sum "$CLANG_TAG" /opt/ccx/runner /opt/ccx/batch-runner /opt/ccx/alloc_shim.o | sha256sum | cut -d' ' -f1)
rust_harness_sha=$(docker run --rm --entrypoint sha256sum "$RUST_TAG" /opt/ccx/runner /opt/ccx/batch-runner /opt/ccx/alloc_shim.o | sha256sum | cut -d' ' -f1)

mkdir -p docker
python3 - "$gcc_digest" "$clang_digest" "$rust_digest" "$gcc_version" "$gcc13_version" "$clang_version" "$clang18_version" "$rust_version" "$gdc_version" "$zig_version" "$odin_version" "$go_version" "$cmake_version" "$ninja_version" "$make_version" "$libc_version" "$BASE_DIGEST" "$DEBIAN_SNAPSHOT" "$gcc_sha" "$gcc13_sha" "$clang_sha" "$clang18_sha" "$rust_sha" "$gdc_sha" "$zig_sha" "$odin_sha" "$go_sha" "$cmake_sha" "$ninja_sha" "$make_sha" "$harness_sha" "$clang_harness_sha" "$rust_harness_sha" "$ZIG_SHA256" "$ODIN_SHA256" "$GO_SHA256" <<'PY'
import hashlib, json, pathlib, sys
(gcc_digest, clang_digest, rust_digest, gcc_version, gcc13_version, clang_version, clang18_version, rust_version,
 gdc_version, zig_version, odin_version, go_version, cmake_version, ninja_version, make_version,
 libc_version, base_digest, debian_snapshot, gcc_sha, gcc13_sha, clang_sha, clang18_sha, rust_sha, gdc_sha,
 zig_sha, odin_sha, go_sha, cmake_sha, ninja_sha, make_sha, harness_sha, clang_harness_sha, rust_harness_sha,
 zig_archive_sha, odin_archive_sha, go_archive_sha) = sys.argv[1:37]

def split_digest(ref):
    image, _, digest = ref.partition("@")
    return image, digest

def file_sha256(path):
    return hashlib.sha256(pathlib.Path(path).read_bytes()).hexdigest()

def environment_fingerprint(image, dockerfile, binaries, context_files, image_harness_sha):
    archive_hashes = {
        "docker/zig-x86_64-linux-0.17.0.tar.xz": zig_archive_sha,
        "docker/odin-linux-amd64-dev-2026-10.tar.gz": odin_archive_sha,
        "docker/go1.26.10.linux-amd64.tar.gz": go_archive_sha,
    }
    identity = {
        "format": "ccx-toolchain-environment-v1",
        "image": image,
        "base_digest": base_digest,
        "apt_snapshot": debian_snapshot,
        "dockerfile_sha256": file_sha256("docker/" + dockerfile),
        "context_sha256": {
            path: archive_hashes[path] if path in archive_hashes else file_sha256(path)
            for path in context_files
        },
        "binary_sha256": binaries,
        "harness_sha256": image_harness_sha,
        "libc": libc_version,
    }
    payload = json.dumps(identity, sort_keys=True, separators=(",", ":")).encode()
    return "sha256:" + hashlib.sha256(payload).hexdigest()

gcc_image, gcc_sha_img = split_digest(gcc_digest)
clang_image, clang_sha_img = split_digest(clang_digest)
rust_image, rust_sha_img = split_digest(rust_digest)

harness_sources = [
    "docker/harness/runner.c",
    "docker/harness/batch_runner.c",
    "docker/harness/alloc_shim.c",
]
common_build_tools = {
    "cmake": {"version": cmake_version, "sha256": cmake_sha},
    "ninja": {"version": ninja_version, "sha256": ninja_sha},
    "make": {"version": make_version, "sha256": make_sha},
}
gcc_environment = environment_fingerprint(
    "gcc",
    "gcc.Dockerfile",
    {
        "gcc": gcc_sha,
        "gcc-13": gcc13_sha,
        "gdc": gdc_sha,
        "zig": zig_sha,
        "go": go_sha,
        **common_build_tools,
    },
    harness_sources + ["docker/zig-x86_64-linux-0.17.0.tar.xz", "docker/go1.26.10.linux-amd64.tar.gz"],
    harness_sha,
)
clang_environment = environment_fingerprint(
    "clang",
    "clang.Dockerfile",
    {"clang": clang_sha, "clang-18": clang18_sha, "odin": odin_sha, **common_build_tools},
    harness_sources + ["docker/odin-linux-amd64-dev-2026-10.tar.gz"],
    clang_harness_sha,
)
rust_environment = environment_fingerprint(
    "rust",
    "rust.Dockerfile",
    {"rustc": rust_sha},
    harness_sources,
    rust_harness_sha,
)
environment_by_image = {
    "ccx-gcc": gcc_environment,
    "ccx-clang": clang_environment,
    "ccx-rust": rust_environment,
}

lock = {
    "base_image": "debian:13-slim",
    "base_digest": base_digest,
    "apt_snapshot": debian_snapshot,
    "generated_by": "scripts/pin-toolchains.sh",
    "harness_sha256": harness_sha,
    "harness_sha256_by_image": {
        "ccx-gcc": harness_sha,
        "ccx-clang": clang_harness_sha,
        "ccx-rust": rust_harness_sha,
    },
    "environment_fingerprints": environment_by_image,
    "build_tools": {
        "cmake": {"version": cmake_version, "binary_sha256": cmake_sha},
        "ninja": {"version": ninja_version, "binary_sha256": ninja_sha},
        "make": {"version": make_version, "binary_sha256": make_sha},
    },
    "toolchains": [
        {
            "name": "gcc",
            "version": gcc_version,
            "image": gcc_image,
            "image_digest": gcc_sha_img,
            "environment_fingerprint": gcc_environment,
            "libc": libc_version,
            "compiler": "gcc",
            "compiler_binaries": ["/usr/bin/gcc", "/usr/bin/g++"],
            "compiler_sha256": gcc_sha,
        },
        {
            "name": "gcc-13",
            "version": gcc13_version,
            "image": gcc_image,
            "image_digest": gcc_sha_img,
            "environment_fingerprint": gcc_environment,
            "libc": libc_version,
            "compiler": "gcc-13",
            "compiler_binaries": ["/usr/bin/gcc-13", "/usr/bin/g++-13"],
            "compiler_sha256": gcc13_sha,
        },
        {
            "name": "clang",
            "version": clang_version,
            "image": clang_image,
            "image_digest": clang_sha_img,
            "environment_fingerprint": clang_environment,
            "libc": libc_version,
            "compiler": "clang",
            "compiler_binaries": ["/usr/bin/clang"],
            "compiler_sha256": clang_sha,
        },
        {
            "name": "clang-18",
            "version": clang18_version,
            "image": clang_image,
            "image_digest": clang_sha_img,
            "environment_fingerprint": clang_environment,
            "libc": libc_version,
            "compiler": "clang-18",
            "compiler_binaries": ["/usr/bin/clang-18"],
            "compiler_sha256": clang18_sha,
        },
        {
            "name": "rust",
            "version": rust_version,
            "image": rust_image,
            "image_digest": rust_sha_img,
            "environment_fingerprint": rust_environment,
            "libc": libc_version,
            "compiler": "rustc",
            "compiler_binaries": ["/usr/bin/rustc"],
            "compiler_sha256": rust_sha,
        },
        {
            "name": "gdc",
            "version": gdc_version,
            "image": gcc_image,
            "image_digest": gcc_sha_img,
            "environment_fingerprint": gcc_environment,
            "libc": libc_version,
            "compiler": "gdc-14",
            "compiler_binaries": ["/usr/bin/gdc-14"],
            "compiler_sha256": gdc_sha,
        },
        {
            "name": "zig",
            "version": zig_version,
            "image": gcc_image,
            "image_digest": gcc_sha_img,
            "environment_fingerprint": gcc_environment,
            "libc": libc_version,
            "compiler": "zig",
            "compiler_binaries": ["/opt/zig/zig"],
            "compiler_sha256": zig_sha,
        },
        {
            "name": "odin",
            "version": odin_version,
            "image": clang_image,
            "image_digest": clang_sha_img,
            "environment_fingerprint": clang_environment,
            "libc": libc_version,
            "compiler": "odin",
            "compiler_binaries": ["/opt/odin/odin"],
            "compiler_sha256": odin_sha,
        },
        {
            "name": "go",
            "version": go_version,
            "image": gcc_image,
            "image_digest": gcc_sha_img,
            "environment_fingerprint": gcc_environment,
            "libc": libc_version,
            "compiler": "go",
            "compiler_binaries": ["/usr/local/go/bin/go"],
            "compiler_sha256": go_sha,
        },
    ],
}

with open("docker/toolchains.lock.json", "w") as fh:
    json.dump(lock, fh, indent=2)
    fh.write("\n")

print(f"gcc   {gcc_version}  compiler={gcc_sha[:16]}")
print(f"gcc-13 {gcc13_version}  compiler={gcc13_sha[:16]}")
print(f"clang {clang_version}  compiler={clang_sha[:16]}")
print(f"clang-18 {clang18_version}  compiler={clang18_sha[:16]}")
print(f"rust  {rust_version}  compiler={rust_sha[:16]}")
print(f"gdc   {gdc_version}  compiler={gdc_sha[:16]}")
print(f"zig   {zig_version}  compiler={zig_sha[:16]}")
print(f"odin  {odin_version}  compiler={odin_sha[:16]}")
print(f"go    {go_version}  compiler={go_sha[:16]}")
print(f"cmake {cmake_version}  binary={cmake_sha[:16]}")
print(f"ninja {ninja_version}  binary={ninja_sha[:16]}")
print(f"make  {make_version}  binary={make_sha[:16]}")
print(f"libc  {libc_version}")
print(f"harness {harness_sha[:16]}")
PY

echo "==> wrote docker/toolchains.lock.json"

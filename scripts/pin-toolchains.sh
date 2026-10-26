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
GCC_TAG="ccx-gcc:pin"
CLANG_TAG="ccx-clang:pin"

echo "==> building $GCC_TAG"
docker build -q -f docker/gcc.Dockerfile -t "$GCC_TAG" docker >/dev/null

echo "==> building $CLANG_TAG"
docker build -q -f docker/clang.Dockerfile -t "$CLANG_TAG" docker >/dev/null

gcc_digest=$(docker inspect --format='{{index .RepoDigests 0}}' "$GCC_TAG" 2>/dev/null || true)
clang_digest=$(docker inspect --format='{{index .RepoDigests 0}}' "$CLANG_TAG" 2>/dev/null || true)

# Locally built images have no RepoDigests until pushed; fall back to the
# image ID, which is itself a content digest and is what `docker run` resolves.
if [[ -z "$gcc_digest" ]]; then
  gcc_digest="ccx-gcc@$(docker inspect --format='{{.Id}}' "$GCC_TAG")"
fi
if [[ -z "$clang_digest" ]]; then
  clang_digest="ccx-clang@$(docker inspect --format='{{.Id}}' "$CLANG_TAG")"
fi

gcc_version=$(docker run --rm --entrypoint gcc "$GCC_TAG" -dumpfullversion -dumpversion | head -1)
# `clang --version` prints a long banner; keep just the numeric version.
clang_version=$(docker run --rm --entrypoint clang "$CLANG_TAG" -dumpversion | head -1)
# `ldd --version` prints "ldd (Debian GLIBC 2.41-12+deb13u4) 2.41"; keep "glibc 2.41".
libc_version=$(docker run --rm --entrypoint ldd "$GCC_TAG" --version | head -1 | sed -E 's/.*GLIBC ([0-9.]+).*/glibc \1/')

# A locally built image ID is a content digest, but it is not stable across
# rebuilds because apt layer timestamps differ. The compiler binaries are,
# though, so hash them: this is the machine-independent identity of the
# toolchain, and it is what a cross-machine determinism check compares.
gcc_sha=$(docker run --rm --entrypoint sha256sum "$GCC_TAG" /usr/bin/gcc /usr/bin/g++ | sha256sum | cut -d' ' -f1)
clang_sha=$(docker run --rm --entrypoint sha256sum "$CLANG_TAG" /usr/bin/clang | sha256sum | cut -d' ' -f1)
harness_sha=$(docker run --rm --entrypoint sha256sum "$GCC_TAG" /opt/ccx/runner /opt/ccx/alloc_shim.o | sha256sum | cut -d' ' -f1)

mkdir -p docker
python3 - "$gcc_digest" "$clang_digest" "$gcc_version" "$clang_version" "$libc_version" "$BASE_DIGEST" "$gcc_sha" "$clang_sha" "$harness_sha" <<'PY'
import json, sys
(gcc_digest, clang_digest, gcc_version, clang_version, libc_version, base_digest,
 gcc_sha, clang_sha, harness_sha) = sys.argv[1:10]

def split_digest(ref):
    image, _, digest = ref.partition("@")
    return image, digest

gcc_image, gcc_sha_img = split_digest(gcc_digest)
clang_image, clang_sha_img = split_digest(clang_digest)

lock = {
    "base_image": "debian:13-slim",
    "base_digest": base_digest,
    "generated_by": "scripts/pin-toolchains.sh",
    "harness_sha256": harness_sha,
    "toolchains": [
        {
            "name": "gcc",
            "version": gcc_version,
            "image": gcc_image,
            "image_digest": gcc_sha_img,
            "libc": libc_version,
            "compiler": "gcc",
            "compiler_sha256": gcc_sha,
        },
        {
            "name": "clang",
            "version": clang_version,
            "image": clang_image,
            "image_digest": clang_sha_img,
            "libc": libc_version,
            "compiler": "clang",
            "compiler_sha256": clang_sha,
        },
    ],
}

with open("docker/toolchains.lock.json", "w") as fh:
    json.dump(lock, fh, indent=2)
    fh.write("\n")

print(f"gcc   {gcc_version}  compiler={gcc_sha[:16]}")
print(f"clang {clang_version}  compiler={clang_sha[:16]}")
print(f"libc  {libc_version}")
print(f"harness {harness_sha[:16]}")
PY

echo "==> wrote docker/toolchains.lock.json"

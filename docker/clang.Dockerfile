# CodeCurrency Clang toolchain image (Layer 2: reproducible environment).
#
# Same base digest as the GCC image, so libc and binutils match and the only
# variable between toolchains is the compiler itself.
FROM debian:13-slim@sha256:62bd423aeee912639ca8ee681c2b762cb3e25137e0a92b20be455898a9f034ad

ENV DEBIAN_FRONTEND=noninteractive

RUN apt-get -qq update && apt-get -qq install -y --no-install-recommends \
        clang \
        lld \
        libc6-dev \
        binutils \
        make \
        valgrind \
        ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# The runner is built with the image's C compiler. On the Clang image that is
# clang, so the harness is exercised by both toolchains.
COPY harness/runner.c /opt/ccx/runner.c
COPY harness/alloc_shim.c /opt/ccx/alloc_shim.c
RUN clang -O2 -std=gnu11 -o /opt/ccx/runner /opt/ccx/runner.c \
    && clang -O2 -std=gnu11 -c -o /opt/ccx/alloc_shim.o /opt/ccx/alloc_shim.c \
    && chmod 0555 /opt/ccx/runner && chmod 0444 /opt/ccx/alloc_shim.o

RUN useradd --create-home --uid 1000 ccx

WORKDIR /work

# CodeCurrency GCC toolchain image (Layer 2: reproducible environment).
#
# Pinned base digest, offline-capable, no network at run time.
FROM debian:13-slim@sha256:62bd423aeee912639ca8ee681c2b762cb3e25137e0a92b20be455898a9f034ad

ENV DEBIAN_FRONTEND=noninteractive

RUN apt-get -qq update && apt-get -qq install -y --no-install-recommends \
        gcc \
        g++ \
        libc6-dev \
        binutils \
        make \
        valgrind \
        ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# The measurement runner and the allocation shim are compiled into the image so
# that no submission can substitute its own copy, and so the harness version is
# pinned by the same digest as the compiler.
COPY harness/runner.c /opt/ccx/runner.c
COPY harness/alloc_shim.c /opt/ccx/alloc_shim.c
RUN gcc -O2 -std=gnu11 -o /opt/ccx/runner /opt/ccx/runner.c \
    && gcc -O2 -std=gnu11 -c -o /opt/ccx/alloc_shim.o /opt/ccx/alloc_shim.c \
    && chmod 0555 /opt/ccx/runner && chmod 0444 /opt/ccx/alloc_shim.o

# A non-root user exists so the sandbox has somewhere to land, though the
# sandbox itself overrides the uid at run time.
RUN useradd --create-home --uid 1000 ccx

WORKDIR /work

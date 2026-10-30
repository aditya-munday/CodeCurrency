# CodeCurrency GCC toolchain image (Layer 2: reproducible environment).
#
# Pinned base digest, offline-capable, no network at run time.
FROM debian:13-slim@sha256:62bd423aeee912639ca8ee681c2b762cb3e25137e0a92b20be455898a9f034ad

ARG DEBIAN_SNAPSHOT=20261020T213255Z
ENV DEBIAN_FRONTEND=noninteractive

RUN printf 'deb [check-valid-until=no] http://snapshot.debian.org/archive/debian/%s/ trixie main\ndeb [check-valid-until=no] http://snapshot.debian.org/archive/debian/%s/ trixie-updates main\ndeb [check-valid-until=no] http://snapshot.debian.org/archive/debian-security/%s/ trixie-security main\n' "$DEBIAN_SNAPSHOT" "$DEBIAN_SNAPSHOT" "$DEBIAN_SNAPSHOT" > /etc/apt/sources.list \
    && rm -f /etc/apt/sources.list.d/debian.sources \
    && apt-get -qq update \
    && apt-get -qq install -y --no-install-recommends \
        gcc=4:14.2.0-1 \
        g++=4:14.2.0-1 \
        gcc-13=13.3.0-16 \
        g++-13=13.3.0-16 \
        gdc-14=14.2.0-19 \
        libc6-dev=2.41-12+deb13u4 \
        binutils=2.44-3 \
        make=4.4.1-2 \
        cmake=3.31.6-2 \
        ninja-build=1.12.1-1 \
        valgrind=1:3.24.0-3 \
        ca-certificates=20250419+deb13u1 \
        curl=8.14.1-2+deb13u7 \
        xz-utils=5.8.1-1+deb13u2 \
    && rm -rf /var/lib/apt/lists/*

COPY zig-x86_64-linux-0.17.0.tar.xz /tmp/zig.tar.xz
RUN echo "1cbe9df9f27e6b78d14ccbca43b6703a404ef79ef1c463de901d7f088d4e2026  /tmp/zig.tar.xz" | sha256sum --check \
    && mkdir -p /opt/zig \
    && tar -xJf /tmp/zig.tar.xz --strip-components=1 -C /opt/zig \
    && ln -s /opt/zig/zig /usr/local/bin/zig \
    && rm /tmp/zig.tar.xz

COPY go1.26.10.linux-amd64.tar.gz /tmp/go.tar.gz
RUN echo "ccfe7ae822d2c2f41f4fe7c956199967a4dac22d0ee0d2b2bfdb440aab3b20c5  /tmp/go.tar.gz" | sha256sum --check \
    && tar -xzf /tmp/go.tar.gz -C /usr/local \
    && ln -s /usr/local/go/bin/go /usr/local/bin/go \
    && rm /tmp/go.tar.gz

# The measurement runner and the allocation shim are compiled into the image so
# that no submission can substitute its own copy, and so the harness version is
# pinned by the same digest as the compiler.
COPY harness/runner.c /opt/ccx/runner.c
COPY harness/batch_runner.c /opt/ccx/batch_runner.c
COPY harness/alloc_shim.c /opt/ccx/alloc_shim.c
RUN gcc -O2 -std=gnu11 -o /opt/ccx/runner /opt/ccx/runner.c \
    && gcc -O2 -std=gnu11 -o /opt/ccx/batch-runner /opt/ccx/batch_runner.c \
    && gcc -O2 -std=gnu11 -c -o /opt/ccx/alloc_shim.o /opt/ccx/alloc_shim.c \
    && chmod 0555 /opt/ccx/runner /opt/ccx/batch-runner && chmod 0444 /opt/ccx/alloc_shim.o

# A non-root user exists so the sandbox has somewhere to land, though the
# sandbox itself overrides the uid at run time.
RUN useradd --create-home --uid 1000 ccx

WORKDIR /work

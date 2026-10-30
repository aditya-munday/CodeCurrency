# CodeCurrency Clang toolchain image (Layer 2: reproducible environment).
#
# Same base digest as the GCC image, so libc and binutils match and the only
# variable between toolchains is the compiler itself.
FROM debian:13-slim@sha256:62bd423aeee912639ca8ee681c2b762cb3e25137e0a92b20be455898a9f034ad

ARG DEBIAN_SNAPSHOT=20261020T213255Z
ENV DEBIAN_FRONTEND=noninteractive

RUN printf 'deb [check-valid-until=no] http://snapshot.debian.org/archive/debian/%s/ trixie main\ndeb [check-valid-until=no] http://snapshot.debian.org/archive/debian/%s/ trixie-updates main\ndeb [check-valid-until=no] http://snapshot.debian.org/archive/debian-security/%s/ trixie-security main\n' "$DEBIAN_SNAPSHOT" "$DEBIAN_SNAPSHOT" "$DEBIAN_SNAPSHOT" > /etc/apt/sources.list \
    && rm -f /etc/apt/sources.list.d/debian.sources \
    && apt-get -qq update \
    && apt-get -qq install -y --no-install-recommends \
        clang=1:19.0-63 \
        clang-18=1:18.1.8-18+b1 \
        lld=1:19.0-63 \
        libc6-dev=2.41-12+deb13u4 \
        binutils=2.44-3 \
        make=4.4.1-2 \
        cmake=3.31.6-2 \
        ninja-build=1.12.1-1 \
        valgrind=1:3.24.0-3 \
        ca-certificates=20250419+deb13u1 \
    && rm -rf /var/lib/apt/lists/*

COPY odin-linux-amd64-dev-2026-10.tar.gz /tmp/odin.tar.gz
ENV ODIN_ROOT=/opt/odin
RUN echo "c3c8b095621fd0c75f7f73e3a0829f1b4d45324225f20ba11ed8dc4da310a8ab  /tmp/odin.tar.gz" | sha256sum --check \
    && mkdir -p /opt/odin \
    && tar -xzf /tmp/odin.tar.gz --strip-components=1 -C /opt/odin \
    && ln -s /opt/odin/odin /usr/local/bin/odin \
    && rm /tmp/odin.tar.gz

# The runner is built with the image's C compiler. On the Clang image that is
# clang, so the harness is exercised by both toolchains.
COPY harness/runner.c /opt/ccx/runner.c
COPY harness/batch_runner.c /opt/ccx/batch_runner.c
COPY harness/alloc_shim.c /opt/ccx/alloc_shim.c
RUN clang -O2 -std=gnu11 -o /opt/ccx/runner /opt/ccx/runner.c \
    && clang -O2 -std=gnu11 -o /opt/ccx/batch-runner /opt/ccx/batch_runner.c \
    && clang -O2 -std=gnu11 -c -o /opt/ccx/alloc_shim.o /opt/ccx/alloc_shim.c \
    && chmod 0555 /opt/ccx/runner /opt/ccx/batch-runner && chmod 0444 /opt/ccx/alloc_shim.o

RUN useradd --create-home --uid 1000 ccx

WORKDIR /work

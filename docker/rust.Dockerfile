FROM debian:13-slim@sha256:62bd423aeee912639ca8ee681c2b762cb3e25137e0a92b20be455898a9f034ad

ARG DEBIAN_SNAPSHOT=20261020T213255Z
ENV DEBIAN_FRONTEND=noninteractive

RUN printf 'deb [check-valid-until=no] http://snapshot.debian.org/archive/debian/%s/ trixie main\ndeb [check-valid-until=no] http://snapshot.debian.org/archive/debian/%s/ trixie-updates main\ndeb [check-valid-until=no] http://snapshot.debian.org/archive/debian-security/%s/ trixie-security main\n' "$DEBIAN_SNAPSHOT" "$DEBIAN_SNAPSHOT" "$DEBIAN_SNAPSHOT" > /etc/apt/sources.list \
    && rm -f /etc/apt/sources.list.d/debian.sources \
    && apt-get -qq update \
    && apt-get -qq install -y --no-install-recommends \
        rustc=1.85.1+dfsg1-1+deb13u1 \
        cargo=1.85.1+dfsg1-1+deb13u1 \
        gcc=4:14.2.0-1 \
        g++=4:14.2.0-1 \
        libc6-dev=2.41-12+deb13u4 \
        binutils=2.44-3 \
        valgrind=1:3.24.0-3 \
        ca-certificates=20250419+deb13u1 \
    && rm -rf /var/lib/apt/lists/*

COPY harness/runner.c /opt/ccx/runner.c
COPY harness/batch_runner.c /opt/ccx/batch_runner.c
COPY harness/alloc_shim.c /opt/ccx/alloc_shim.c
RUN gcc -O2 -std=gnu11 -o /opt/ccx/runner /opt/ccx/runner.c \
    && gcc -O2 -std=gnu11 -o /opt/ccx/batch-runner /opt/ccx/batch_runner.c \
    && gcc -O2 -std=gnu11 -c -o /opt/ccx/alloc_shim.o /opt/ccx/alloc_shim.c \
    && chmod 0555 /opt/ccx/runner /opt/ccx/batch-runner && chmod 0444 /opt/ccx/alloc_shim.o

RUN useradd --create-home --uid 1000 ccx

WORKDIR /work

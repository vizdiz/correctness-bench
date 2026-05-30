# wrk2 (Gil Tene's coordinated-omission-correct fork of wrk) for Linux.
# Gate #1's oracle.
#
# wrk2's bundled LuaJIT 2.0 has no arm64 port. This image swaps it for upstream
# LuaJIT 2.1 and patches two small source incompatibilities so wrk2 builds
# natively for whatever arch you build on — no QEMU/Rosetta, no segfaults.
#
# Usage:
#   docker build -t correctness-bench-wrk2 tools/wrk2
#   docker compose --profile bench run --rm wrk2 -t4 -c100 -d60s -R2000 \
#       --latency http://mock:8080/api?mode=healthy\&base_latency_ms=20

FROM debian:bookworm-slim AS build
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates git build-essential libssl-dev zlib1g-dev python3 \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY build.sh /tmp/build.sh
RUN sh /tmp/build.sh

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates libssl3 zlib1g \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/wrk /usr/local/bin/wrk
ENTRYPOINT ["wrk"]

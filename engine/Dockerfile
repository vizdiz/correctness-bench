# Single-worker engine, in a Linux container, for gate-#1 parity with wrk2
# (both inside the compose bridge net hitting the mock service directly).
#
# Build context is the REPO ROOT (Cargo.toml references ../mock as a dev-dep):
#   docker build -t correctness-bench-engine -f engine/Dockerfile .

FROM rust:1-slim AS build
WORKDIR /src
# Pull in just the manifests we need (engine + its mock dev-dep) so cache stays
# tight as code changes.
COPY engine/Cargo.toml engine/Cargo.lock* /src/engine/
COPY mock/Cargo.toml mock/Cargo.lock* /src/mock/
COPY engine/src /src/engine/src
COPY mock/src /src/mock/src
WORKDIR /src/engine
RUN cargo build --release --bin engine-worker

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/engine/target/release/engine-worker /usr/local/bin/engine-worker
ENTRYPOINT ["/usr/local/bin/engine-worker"]

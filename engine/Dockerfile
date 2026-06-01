# Engine image. Ships THREE binaries; the compose service picks which one to
# run via `entrypoint:` override:
#   engine-worker  - CLI-driven standalone worker (used by CLI + MCP today)
#   coordinator    - gRPC + admin HTTP for the fleet
#   worker_node    - gRPC peer of coordinator (fleet member)
#
# Build context is the REPO ROOT (Cargo.toml references ../mock as a dev-dep,
# and build.rs reads ../contracts/bench.proto):
#   docker build -t correctness-bench-engine -f engine/Dockerfile .

FROM rust:1-slim AS build
RUN apt-get update \
    && apt-get install -y --no-install-recommends protobuf-compiler ca-certificates \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY engine/Cargo.toml engine/Cargo.lock* /src/engine/
COPY engine/build.rs /src/engine/
COPY mock/Cargo.toml mock/Cargo.lock* /src/mock/
COPY contracts /src/contracts
COPY engine/src /src/engine/src
COPY mock/src /src/mock/src
WORKDIR /src/engine
RUN cargo build --release --bin engine-worker --bin coordinator --bin worker_node

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/engine/target/release/engine-worker /usr/local/bin/engine-worker
COPY --from=build /src/engine/target/release/coordinator   /usr/local/bin/coordinator
COPY --from=build /src/engine/target/release/worker_node   /usr/local/bin/worker_node
ENTRYPOINT ["/usr/local/bin/engine-worker"]

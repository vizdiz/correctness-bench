# Multi-stage build for the mock target.
# NOTE: authored without a Docker runtime available this session — NOT yet
# `docker build`-verified. The binary itself is verified natively (cargo test +
# curl, see docs/BUILD_LOG.md).

FROM rust:1-slim AS build
WORKDIR /app
# Cache deps: copy manifests, build a stub, then the real sources.
COPY Cargo.toml Cargo.lock* ./
RUN mkdir -p src && echo 'fn main() {}' > src/main.rs && echo '' > src/lib.rs \
    && cargo build --release --bin mock || true
COPY src ./src
RUN touch src/main.rs src/lib.rs && cargo build --release --bin mock

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends curl ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /app/target/release/mock /usr/local/bin/mock
ENV MOCK_ADDR=0.0.0.0:8080
EXPOSE 8080
HEALTHCHECK --interval=5s --timeout=3s --retries=5 \
    CMD curl -fsS http://localhost:8080/healthz || exit 1
ENTRYPOINT ["/usr/local/bin/mock"]

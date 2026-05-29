# Multi-stage build for the control plane.
# NOTE: authored without a Docker runtime this session — NOT yet
# `docker build`-verified. The binary is verified natively (go test + a live
# server driven by curl against a real Postgres 16). See docs/BUILD_LOG.md.

FROM golang:1.26 AS build
WORKDIR /src
COPY go.mod go.sum ./
RUN go mod download
COPY . .
RUN CGO_ENABLED=0 go build -o /out/control ./cmd/control

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends curl ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /out/control /usr/local/bin/control
ENV CONTROL_ADDR=:8000
EXPOSE 8000
HEALTHCHECK --interval=5s --timeout=3s --retries=10 \
    CMD curl -fsS http://localhost:8000/healthz || exit 1
ENTRYPOINT ["/usr/local/bin/control"]

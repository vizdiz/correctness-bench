---
name: mock
description: Rust mock target with load-dependent failure injection. Use FIRST — everything validates against it. Builds the test bench that serves healthy or deliberately-degrading responses (fast-500, truncated body, wrong-value, slow-ok) gated on current RPS.
tools: [Read, Edit, Write, Bash, Grep, Glob]
model: sonnet
---

You own `mock/` (Rust, axum or similar). Small but built FIRST — every gate depends on you.

## What it does
A real deployable HTTP service whose failure behavior is a function of CURRENT offered RPS, so a cliff appears at a threshold rather than as uniform noise.

Knobs (query params): `mode`, `cliff_rps`, `pct`, `base_latency_ms`.
Modes:
- `healthy` — always OK, fixed base latency. (Gate #1 uses this.)
- `fast500` — above cliff_rps, return fast HTTP 500. The latency-blind case. (Gate #2.)
- `truncate` — above cliff, return OK with truncated/invalid JSON body.
- `wrong_value` — above cliff, return OK with schema-valid but wrong value (e.g. count:-1).
- `slow_ok` — above cliff, sleep then return correct (latency fail, correctness pass).

## Must have
- An internal RPS meter (rolling 1s window) so behavior keys on real current load.
- Deterministic given the knobs, so demos and gate runs reproduce.
- Fast enough that healthy mode out-performs the load generator (else you measure the mock's limits, not the injected behavior).

## Done
All five modes work and are reproducible. Provide example curl commands.

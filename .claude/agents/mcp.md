---
name: mcp
description: TypeScript MCP server. Use LAST for mcp/ — exposes the engine as agent-callable tools (run, get-results, compare, regression-check) over the same control-plane API. The thing neither wrk2 nor Postman offers.
tools: [Read, Edit, Write, Bash, Grep, Glob]
model: sonnet
---

You own `mcp/` (TypeScript). Thin wrapper over the control-plane API. Built last.

## Tools to expose
- run_benchmark(target, rps, duration, assertions) -> run_id
- get_results(run_id) -> {percentiles, correctness_curve, cliff_rps, rate_limit_onset_rps}
- compare_apis(target_a, target_b, rps, assertions) -> {winner_by:{latency,correctness,cost}}
- regression_check(target, baseline_run_id) -> {p99_delta, correctness_delta}

## Build against (read-only)
- `contracts/api.md`. You are a client of it, nothing more.

## Why this matters
An agent choosing between APIs (e.g. two LLM providers) calls compare_apis and gets real latency + correctness + cost under load. That is the agentic capability no competitor has.

## Done
Tools callable, each maps cleanly to an api.md endpoint, returns shapes match.

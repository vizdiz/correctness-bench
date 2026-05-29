// mcp — MCP server exposing the benchmarker as agent tools (built LAST).
// Tools to expose (all thin wrappers over contracts/api.md):
//   run_benchmark(target, rps, duration, assertions) -> run_id
//   get_results(run_id) -> { percentiles, correctness_curve, cliff_rps, rate_limit_onset_rps }
//   compare_apis(target_a, target_b, rps, assertions) -> { winner_by: { latency, correctness, cost } }
//   regression_check(target, baseline_run_id) -> { p99_delta, correctness_delta }
// See .claude/agents/mcp.md. Stub only during scaffolding.

export {};

console.error("mcp server: not yet implemented (stub). See .claude/agents/mcp.md");
process.exit(2);

# Build kit — how to use this

Drop these into your monorepo root. They turn the design doc into something Claude Code agents can build without drifting.

## Layout
```
CLAUDE.md                      orchestrator brief (root)
contracts/
  bench.proto                  worker<->coordinator gRPC (FROZEN)
  api.md                       control-plane REST+SSE (FROZEN)
  schema.sql                   Postgres+Timescale (FROZEN)
gates/
  gate1_wrk2_agreement.md      engine must match wrk2 (BLOCKS ALL)
  gate2_oracle.md              injected % == reported %
  gate3_merge.md               half+half == full
.claude/agents/
  engine.md   mock.md   control.md   web.md   cli.md   mcp.md
```

## The idea
- **Contracts are frozen.** Every agent builds against them as read-only truth. This is what lets agents work in parallel without integration hell. If a contract is wrong, the human changes it, not an agent.
- **Gates are acceptance tests.** An agent isn't done until its gate passes, with the command and output shown. Objective, not "looks done."
- **One critical-path agent first.** The `engine` agent runs mostly alone until gate #1 is green, because everything depends on it and parallel agents burn quota linearly. `mock` is built first (tiny, everything validates against it).

## Suggested flow in Claude Code
1. `mock` agent builds the bench. Verify modes with curl.
2. `engine` agent builds the single worker. **It must pass gate #1 before anything else.** Use the stock wrk2 binary as the agreement oracle; clone the wrk2 source locally and read it as a reference for the COO port.
3. Once the engine streams real data, dispatch `control` and `web` in parallel (separate sessions / agent view). They share `contracts/api.md`.
4. `engine` adds the coordinator + 2nd worker, passes gate #3.
5. `control` adds the offload eval pool; `engine`/`control` wire up gate #2 end to end.
6. `cli` then `mcp` last — thin clients of the API.

## Cost discipline
- Cheaper models for thin work (mock/cli/mcp on sonnet), opus for engine/control/web.
- `/clear` between unrelated dispatches.
- Don't run four agents when three are blocked on the engine. Sequence early, parallelize after gate #1.

## Built-in agents worth using
- **Explore** (read-only, cheap): research the cloned wrk2 source before the engine agent ports COO. "Find and explain usec_to_next_send and the latency recording path."
- **Plan**: gather context before a big dispatch without polluting the builder's window.

## Phasing
Phase 1 (~1.5 weeks): demoable v1 — engine + fleet + control + single-page web + mock + CLI. Gates 1, 2, 3 green. Logs to stdout. User-declared cost ceiling.

Phase 2 (after Phase 1 ships, sooner if early): MCP server, OpenTelemetry, centralized log aggregation, screenshot regression, multi-page UI evolution.

Permanently out of scope for this project: continuous monitoring/alerting, geo probes, user auth, threshold gating in CLI.

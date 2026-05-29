// cli — thin client of the control plane (built late; see .claude/agents/cli.md)
//
// Planned: `bench --target X --assert spec.json [--rps N] [--duration S]`
//   -> POST /v1/runs, stream progress via SSE, print dashboard link.
// Exit codes: 0 = run completed, 1 = run failed, 2 = bad args / control plane unreachable.

fn main() {
    eprintln!("bench-cli: not yet implemented (stub). See .claude/agents/cli.md");
    std::process::exit(2);
}

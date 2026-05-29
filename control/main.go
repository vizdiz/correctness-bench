package main

// control — Go control plane (lifecycle, REST + SSE, credential custody).
// Built against contracts/api.md and contracts/schema.sql.
// TODO(phase3): wire router, pgx pool, migrations, and the v1 endpoints.

func main() {
	// TODO: replaced in Phase 3 by cmd/control wiring (router + pgx + handlers).
	panic("control plane not yet implemented — see .claude/agents/control.md")
}

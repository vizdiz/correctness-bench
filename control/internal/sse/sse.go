// Package sse will carry the live run stream (tick/status/done events) per
// contracts/api.md GET /v1/runs/{id}/stream.
//
// SCAFFOLD ONLY. The streaming implementation depends on real engine data
// flowing through the coordinator and is intentionally not built yet (see
// docs/PLAN.md C.2). The Broker below documents the intended shape so the
// dependency and event types are pinned, but no endpoint is wired to it.
package sse

import "time"

// Event is one SSE message. Each carries an id (the tick number) so clients can
// resume with Last-Event-ID.
type Event struct {
	ID   uint64 // tick number; used as the SSE `id:` field
	Type string // "tick" | "status" | "warning" | "done"
	Data []byte // JSON payload
	TS   time.Time
}

// Broker will fan run events out to connected SSE clients with a short replay
// buffer (>=30s) for Last-Event-ID resume. Not implemented yet.
type Broker struct{}

// TODO(phase: engine-integration): implement Subscribe/Publish + replay buffer
// once the coordinator streams merged ticks into the control plane.

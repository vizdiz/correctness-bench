// Package sse holds the in-memory tick broker that bridges engine workers
// (push side: POST /v1/_internal/runs/:id/tick) to web/CLI subscribers
// (pull side: GET /v1/runs/:id/stream as Server-Sent Events).
//
// This is the v0 transport. The frozen contract for worker↔coordinator is
// `contracts/bench.proto` (gRPC) — once the engine speaks bench.proto to a
// real coordinator, the coordinator forwards into this same broker; the SSE
// surface to web/CLI does not change.
package sse

import (
	"sync"
	"time"
)

// Tick is what engine workers push. Shape is a subset of api.md's SSE tick
// event so that growing fields toward full api.md parity is additive.
type Tick struct {
	ElapsedS         uint64            `json:"elapsed_s"`
	AchievedRps1S    float64           `json:"achieved_rps_1s"`
	CompletedTotal   uint64            `json:"completed_total"`
	PassTotal        uint64            `json:"pass_total"`
	FailStatusTotal  uint64            `json:"fail_status_total"`
	ThisTick         ThisTick          `json:"this_tick"`
	PercentilesSoFar PercentilesSoFar  `json:"percentiles_so_far"`
	Buckets          []Bucket          `json:"buckets,omitempty"`
	Sampled          []SampledResponse `json:"sampled,omitempty"`
	TS               time.Time         `json:"ts,omitempty"` // server-stamped on ingest
}

// SampledResponse is one captured response body shipped by the worker for the
// control plane's offload eval pool. Shape matches what engine emits today.
type SampledResponse struct {
	RpsAtSend      float64 `json:"rps_at_send"`
	Status         uint16  `json:"status"`
	CorrectedLatUS uint64  `json:"corrected_lat_us"`
	// Body is UTF-8 lossy JSON-ish today; binary needs base64 once we ship
	// beyond JSON APIs.
	Body          string `json:"body"`
	BodyTruncated bool   `json:"body_truncated"`
}

// Bucket is one per-RPS correctness bucket. api.md SSE tick `buckets` entry.
type Bucket struct {
	RpsLo           float64 `json:"rps_lo"`
	RpsHi           float64 `json:"rps_hi"`
	Total           uint64  `json:"total"`
	Pass            uint64  `json:"pass"`
	FailStatus      uint64  `json:"fail_status"`
	FailLatency     uint64  `json:"fail_latency"`
	FailSize        uint64  `json:"fail_size"`
	FailContentType uint64  `json:"fail_content_type"`
}

// ThisTick holds the counts attributable to JUST the last 1 s window. Per-tick
// pass rate (pass / total) is what the web's cliff sparkline plots.
type ThisTick struct {
	Total           uint64 `json:"total"`
	Pass            uint64 `json:"pass"`
	FailStatus      uint64 `json:"fail_status"`
	FailLatency     uint64 `json:"fail_latency"`
	FailSize        uint64 `json:"fail_size"`
	FailContentType uint64 `json:"fail_content_type"`
}

// PercentilesSoFar is the running corrected p50/p99 latency snapshot. api.md
// names this `percentiles_so_far`.
type PercentilesSoFar struct {
	P50US uint64 `json:"p50_us"`
	P99US uint64 `json:"p99_us"`
}

// Broker fans run-scoped Ticks out to N concurrent subscribers per run.
// Subscriber channels are buffered; a slow subscriber drops missed ticks
// rather than back-pressuring the publisher (the hard rule: a slow consumer
// must never stall the data path).
type Broker struct {
	mu   sync.RWMutex
	subs map[string][]chan Tick
}

func NewBroker() *Broker {
	return &Broker{subs: make(map[string][]chan Tick)}
}

const subBuffer = 16

// Subscribe returns a channel that receives all ticks published for runID
// from this point forward, plus a cleanup func the caller must defer.
func (b *Broker) Subscribe(runID string) (<-chan Tick, func()) {
	ch := make(chan Tick, subBuffer)
	b.mu.Lock()
	b.subs[runID] = append(b.subs[runID], ch)
	b.mu.Unlock()
	cleanup := func() {
		b.mu.Lock()
		defer b.mu.Unlock()
		list := b.subs[runID]
		for i, c := range list {
			if c == ch {
				b.subs[runID] = append(list[:i], list[i+1:]...)
				break
			}
		}
		if len(b.subs[runID]) == 0 {
			delete(b.subs, runID)
		}
		close(ch)
	}
	return ch, cleanup
}

// Publish delivers tick to every current subscriber of runID. Slow subscribers
// (full buffer) drop the tick rather than blocking.
func (b *Broker) Publish(runID string, t Tick) {
	b.mu.RLock()
	defer b.mu.RUnlock()
	for _, ch := range b.subs[runID] {
		select {
		case ch <- t:
		default:
			// drop — never block the publisher
		}
	}
}

// SubscriberCount returns the number of active subscribers for runID. Useful
// for tests and (eventually) telemetry.
func (b *Broker) SubscriberCount(runID string) int {
	b.mu.RLock()
	defer b.mu.RUnlock()
	return len(b.subs[runID])
}

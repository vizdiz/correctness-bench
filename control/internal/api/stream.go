package api

import (
	"encoding/json"
	"fmt"
	"net/http"
	"time"

	"github.com/go-chi/chi/v5"
	"github.com/google/uuid"

	"github.com/vizdiz/correctness-bench/control/internal/sse"
)

// IngestTick accepts a single Tick payload from a worker and fans it out to
// every active SSE subscriber for that run. v0 transport — the frozen
// worker↔coordinator contract is bench.proto (gRPC); this HTTP path stands in
// until the coordinator is built.
//   POST /v1/_internal/runs/{id}/tick
func (s *Server) IngestTick(w http.ResponseWriter, r *http.Request) {
	id := chi.URLParam(r, "id")
	if _, err := uuid.Parse(id); err != nil {
		writeError(w, http.StatusNotFound, CodeNotFound, "run not found", "")
		return
	}
	var tick sse.Tick
	if err := json.NewDecoder(r.Body).Decode(&tick); err != nil {
		writeError(w, http.StatusBadRequest, CodeInvalidRunSpec, "tick payload not valid JSON", "")
		return
	}
	tick.TS = time.Now().UTC()
	s.Broker.Publish(id, tick)
	w.WriteHeader(http.StatusNoContent)
}

// StreamRun is the api.md SSE endpoint:
//   GET /v1/runs/{id}/stream
// Each event has type `tick`, id = elapsed_s, and a JSON data payload.
// The connection survives until the client disconnects or the request context
// is cancelled. A `keepalive` heartbeat goes out every 15 s so intermediate
// proxies don't reap the connection.
func (s *Server) StreamRun(w http.ResponseWriter, r *http.Request) {
	id := chi.URLParam(r, "id")
	if _, err := uuid.Parse(id); err != nil {
		writeError(w, http.StatusNotFound, CodeNotFound, "run not found", "")
		return
	}

	flusher, ok := w.(http.Flusher)
	if !ok {
		writeError(w, http.StatusInternalServerError, CodeInternal, "streaming not supported", "")
		return
	}

	w.Header().Set("Content-Type", "text/event-stream")
	w.Header().Set("Cache-Control", "no-cache")
	w.Header().Set("Connection", "keep-alive")
	w.Header().Set("X-Accel-Buffering", "no") // disable nginx-style proxy buffering
	w.WriteHeader(http.StatusOK)

	ch, cleanup := s.Broker.Subscribe(id)
	defer cleanup()

	// Initial comment so the client confirms it's connected.
	fmt.Fprintf(w, ": connected to run %s\n\n", id)
	flusher.Flush()

	keepalive := time.NewTicker(15 * time.Second)
	defer keepalive.Stop()

	for {
		select {
		case tick, open := <-ch:
			if !open {
				return
			}
			writeTickEvent(w, tick)
			flusher.Flush()
		case <-keepalive.C:
			fmt.Fprint(w, ": keepalive\n\n")
			flusher.Flush()
		case <-r.Context().Done():
			return
		}
	}
}

func writeTickEvent(w http.ResponseWriter, t sse.Tick) {
	payload, err := json.Marshal(t)
	if err != nil {
		return
	}
	// `event:` lets clients dispatch on type; `id:` lets them resume.
	fmt.Fprintf(w, "event: tick\nid: %d\ndata: %s\n\n", t.ElapsedS, payload)
}


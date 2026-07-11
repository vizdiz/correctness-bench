package api

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"time"

	"github.com/go-chi/chi/v5"
	"github.com/google/uuid"

	"github.com/vizdiz/correctness-bench/control/internal/offload"
	"github.com/vizdiz/correctness-bench/control/internal/sse"
	"github.com/vizdiz/correctness-bench/control/internal/store"
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

	// Offload eval: evaluate every sampled body against this run's spec.
	// Async so the ingest path isn't blocked on the DB write; spawning N
	// goroutines per tick is fine - sample_every_n caps N in practice.
	if len(tick.Sampled) > 0 && s.Offload != nil {
		for _, sample := range tick.Sampled {
			sample := sample
			go s.evalOneSample(id, sample)
		}
	}

	w.WriteHeader(http.StatusNoContent)
}

// evalOneSample runs a single sampled response through the offload evaluator
// and persists the verdict. Errors are logged but never surfaced - the engine
// shouldn't slow down because the eval pool is unhealthy.
func (s *Server) evalOneSample(runID string, sample sse.SampledResponse) {
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()

	eval, err := s.Offload.Get(ctx, runID)
	if err != nil {
		s.Log.Warn("offload spec fetch", "run_id", runID, "err", err.Error())
		return
	}
	if eval == nil {
		// No offload tier configured for this run - nothing to evaluate.
		return
	}

	body := []byte(sample.Body)
	verdict, details := eval.Evaluate(body)
	var detailsJSON string
	if details != "" {
		j, _ := json.Marshal(map[string]string{"reason": details})
		detailsJSON = string(j)
	}
	if err := s.Store.InsertOffloadVerdict(ctx, store.InsertOffloadVerdictParams{
		RunID:          runID,
		RpsAtSend:      sample.RpsAtSend,
		Status:         int32(sample.Status),
		CorrectedLatUS: int64(sample.CorrectedLatUS),
		Verdict:        string(verdict),
		DetailsJSON:    detailsJSON,
		BodySize:       int32(len(body)),
		BodySHA256:     store.SHA256Body(body),
		Body:           nil, // not retaining bodies by default
	}); err != nil {
		s.Log.Warn("offload verdict persist", "run_id", runID, "err", err.Error())
	}
}

// suppress unused-import warning if all callers go away in the future.
var _ = offload.VerdictPass

// FinalizeRunRequest is the body of POST /v1/_internal/runs/:id/finalize.
// Engine workers push this once after their run loop exits to fill in the
// run's final percentiles, totals, and lifecycle status. All percentile fields
// are microseconds (corrected).
type FinalizeRunRequest struct {
	EffectiveRPS  float64  `json:"effective_rps"`
	P50US         int64    `json:"p50_us"`
	P95US         int64    `json:"p95_us"`
	P99US         int64    `json:"p99_us"`
	P999US        int64    `json:"p999_us"`
	TotalRequests int64    `json:"total_requests"`
	TotalPass     int64    `json:"total_pass"`
	CliffRPS      *float64 `json:"cliff_rps,omitempty"`
	Status        string   `json:"status,omitempty"`        // default "completed"
	StatusReason  string   `json:"status_reason,omitempty"` // optional
	// Base64-encoded V2-deflate corrected HDR histogram. Persisted to
	// runs.final_corrected_hist and served verbatim by GET /v1/runs/:id/histogram.
	CorrectedHistB64 string `json:"corrected_hist_b64,omitempty"`
	// Base64-encoded V2-deflate uncorrected HDR histogram. Persisted to
	// runs.final_uncorrected_hist; (corrected - uncorrected) is the COO delta.
	UncorrectedHistB64 string `json:"uncorrected_hist_b64,omitempty"`
}

// FinalizeRun: POST /v1/_internal/runs/{id}/finalize. Internal-only endpoint
// the engine (worker binary or coordinator) calls after its run loop ends.
// Writes finals + transitions status, idempotent on retry.
func (s *Server) FinalizeRun(w http.ResponseWriter, r *http.Request) {
	id := chi.URLParam(r, "id")
	if _, err := uuid.Parse(id); err != nil {
		writeError(w, http.StatusNotFound, CodeNotFound, "run not found", "")
		return
	}
	var req FinalizeRunRequest
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeError(w, http.StatusBadRequest, CodeInvalidRunSpec, "finalize payload not valid JSON", "")
		return
	}
	status := req.Status
	if status == "" {
		status = "completed"
	}
	switch status {
	case "completed", "failed", "aborted":
	default:
		writeError(w, http.StatusBadRequest, CodeInvalidRunSpec, "status must be completed|failed|aborted", "status")
		return
	}
	var histBytes []byte
	if req.CorrectedHistB64 != "" {
		decoded, err := base64.StdEncoding.DecodeString(req.CorrectedHistB64)
		if err != nil {
			writeError(w, http.StatusBadRequest, CodeInvalidRunSpec,
				"corrected_hist_b64 is not valid base64", "corrected_hist_b64")
			return
		}
		histBytes = decoded
	}
	var uncorrectedBytes []byte
	if req.UncorrectedHistB64 != "" {
		decoded, err := base64.StdEncoding.DecodeString(req.UncorrectedHistB64)
		if err != nil {
			writeError(w, http.StatusBadRequest, CodeInvalidRunSpec,
				"uncorrected_hist_b64 is not valid base64", "uncorrected_hist_b64")
			return
		}
		uncorrectedBytes = decoded
	}
	if err := s.Store.FinalizeRun(r.Context(), id, store.FinalizeRunParams{
		EffectiveRPS:         req.EffectiveRPS,
		P50US:                req.P50US,
		P95US:                req.P95US,
		P99US:                req.P99US,
		P999US:               req.P999US,
		TotalRequests:        req.TotalRequests,
		TotalPass:            req.TotalPass,
		CliffRPS:             req.CliffRPS,
		Status:               status,
		StatusReason:         req.StatusReason,
		CorrectedHistBytes:   histBytes,
		UncorrectedHistBytes: uncorrectedBytes,
	}); err != nil {
		if errors.Is(err, store.ErrNotFound) {
			writeError(w, http.StatusNotFound, CodeNotFound, "run not found", "")
			return
		}
		s.internal(w, "finalize run", err)
		return
	}
	s.Log.Info("run finalized",
		"run_id", id, "status", status,
		"effective_rps", req.EffectiveRPS,
		"p99_us", req.P99US,
		"total_requests", req.TotalRequests,
		"total_pass", req.TotalPass,
	)
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


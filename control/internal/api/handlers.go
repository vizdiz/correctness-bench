package api

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"errors"
	"io"
	"log/slog"
	"net/http"
	"strconv"
	"time"

	"github.com/go-chi/chi/v5"
	"github.com/google/uuid"

	"github.com/vizdiz/correctness-bench/control/internal/offload"
	"github.com/vizdiz/correctness-bench/control/internal/sse"
	"github.com/vizdiz/correctness-bench/control/internal/store"
)

// Server holds handler dependencies. The logger is deliberately only ever given
// non-secret fields — target headers and bodies never pass through it.
type Server struct {
	Store   *store.Store
	Log     *slog.Logger
	Broker  *sse.Broker
	Offload *offload.Cache
	// CoordinatorURL is the coordinator admin base; control dispatches runs there.
	// SelfURL is control's own base URL as the coordinator reaches it (for the
	// tick/finalize callbacks). Empty CoordinatorURL disables auto-dispatch.
	CoordinatorURL string
	SelfURL        string
	// dispatch client with no timeout — /admin/runs blocks for the run duration.
	dispatchClient *http.Client
}

func NewServer(s *store.Store, log *slog.Logger, coordinatorURL, selfURL string) *Server {
	if log == nil {
		log = slog.Default()
	}
	cache := offload.NewCache(func(ctx context.Context, runID string) (offload.Spec, error) {
		raw, err := s.GetAssertSpec(ctx, runID)
		if err != nil {
			return offload.Spec{}, err
		}
		return offload.ParseSpecFromAssert(raw)
	})
	return &Server{
		Store:          s,
		Log:            log,
		Broker:         sse.NewBroker(),
		Offload:        cache,
		CoordinatorURL: coordinatorURL,
		SelfURL:        selfURL,
		dispatchClient: &http.Client{},
	}
}

const maxBodyBytes = 1 << 20 // 1 MiB cap on the POST /v1/runs JSON

// CreateRun: POST /v1/runs — validate, scrub creds, insert queued, return run_id.
func (s *Server) CreateRun(w http.ResponseWriter, r *http.Request) {
	var req CreateRunRequest
	dec := json.NewDecoder(io.LimitReader(r.Body, maxBodyBytes))
	if err := dec.Decode(&req); err != nil {
		writeError(w, http.StatusBadRequest, CodeInvalidRunSpec, "request body is not valid JSON", "")
		return
	}

	if ve := validateCreate(&req); ve != nil {
		writeError(w, http.StatusBadRequest, CodeInvalidRunSpec, ve.Message, ve.Field)
		return
	}

	// Decode body to record its SIZE only — the body itself is not persisted.
	var bodySize *int
	if req.Target.BodyBase64 != "" {
		raw, err := base64.StdEncoding.DecodeString(req.Target.BodyBase64)
		if err != nil {
			writeError(w, http.StatusBadRequest, CodeInvalidRunSpec, "target.body_base64 is not valid base64", "target.body_base64")
			return
		}
		n := len(raw)
		bodySize = &n
	}

	// Scrub credentials BEFORE anything is persisted or logged.
	scrubbed := ScrubHeaders(req.Target.Headers)
	headersJSON, err := json.Marshal(scrubbed)
	if err != nil {
		s.internal(w, "marshal headers", err)
		return
	}

	var namePtr *string
	if req.Name != "" {
		namePtr = &req.Name
	}

	p := store.InsertRunParams{
		Name:                namePtr,
		TargetURL:           req.Target.URL,
		TargetMethod:        req.Target.Method,
		HeadersRedactedJSON: string(headersJSON),
		BodySize:            bodySize,
		TargetRPS:           req.TargetRPS,
		DurationS:           req.DurationS,
		WarmupS:             req.WarmupS,
		LoadModel:           req.LoadModel,
		Connections:         req.Connections,
		Keepalive:           *req.Keepalive,
		AssertJSON:          rawOrEmpty(req.Assert),
		RateLimitJSON:       rawOrEmpty(req.RateLimitPolicy),
		EstimatedCostUSD:    req.EstimatedCostUSD,
		CostPerRequestUSD:   req.CostPerRequestUSD,
	}

	id, _, err := s.Store.InsertRun(r.Context(), p)
	if err != nil {
		s.internal(w, "insert run", err)
		return
	}

	// Safe log: identifiers and load shape only. NEVER headers or body.
	s.Log.Info("run created",
		"run_id", id, "url", req.Target.URL, "method", req.Target.Method,
		"target_rps", req.TargetRPS, "duration_s", req.DurationS)

	// Fire the fleet. The request (incl. the in-memory, un-scrubbed target
	// headers) is handed to the dispatcher, which POSTs it to the coordinator
	// and streams ticks/finalize back. Async: /admin/runs blocks for the whole
	// run. A copy is passed so it outlives this handler.
	status := "queued"
	if s.CoordinatorURL != "" {
		reqCopy := req
		go s.dispatchToCoordinator(id, &reqCopy)
		status = "running"
	}

	writeJSON(w, http.StatusCreated, CreateRunResponse{
		RunID:            id,
		Status:           status,
		EstimatedCostUSD: req.EstimatedCostUSD,
	})
}

// GetRun: GET /v1/runs/:id — never includes target.headers.
func (s *Server) GetRun(w http.ResponseWriter, r *http.Request) {
	id := chi.URLParam(r, "id")
	if _, err := uuid.Parse(id); err != nil {
		writeError(w, http.StatusNotFound, CodeNotFound, "run not found", "")
		return
	}
	row, err := s.Store.GetRun(r.Context(), id)
	if errors.Is(err, store.ErrNotFound) {
		writeError(w, http.StatusNotFound, CodeNotFound, "run not found", "")
		return
	}
	if err != nil {
		s.internal(w, "get run", err)
		return
	}
	view := runViewFrom(row)
	// Attach offload verdict counts when at least one row exists. Skipped for
	// runs without an offload tier so the API surface stays clean.
	if counts, err := s.Store.OffloadCountsForRun(r.Context(), id); err == nil {
		if counts.Pass+counts.FailSchema+counts.FailValue+counts.FailRegex > 0 {
			view.Offload = &OffloadCountsView{
				Pass:       counts.Pass,
				FailSchema: counts.FailSchema,
				FailValue:  counts.FailValue,
				FailRegex:  counts.FailRegex,
			}
		}
	} else {
		s.Log.Warn("offload counts read", "run_id", id, "err", err.Error())
	}
	writeJSON(w, http.StatusOK, view)
}

// ListRuns: GET /v1/runs?status=&limit=&cursor= — cursor-based pagination.
func (s *Server) ListRuns(w http.ResponseWriter, r *http.Request) {
	q := r.URL.Query()
	status := q.Get("status")
	cursor := q.Get("cursor")
	limit := 20
	if l := q.Get("limit"); l != "" {
		if n, err := strconv.Atoi(l); err == nil {
			limit = n
		}
	}

	rows, next, err := s.Store.ListRuns(r.Context(), status, limit, cursor)
	if err != nil {
		// A bad cursor is a client error, not a server fault.
		writeError(w, http.StatusBadRequest, CodeInvalidRunSpec, "invalid cursor", "cursor")
		return
	}

	summaries := make([]RunSummary, 0, len(rows))
	for _, row := range rows {
		summaries = append(summaries, RunSummary{
			RunID:     row.ID,
			Name:      derefStr(row.Name),
			Status:    row.Status,
			TargetRPS: row.TargetRPS,
			DurationS: row.DurationS,
			CreatedAt: row.CreatedAt.UTC().Format(time.RFC3339),
		})
	}
	var nextPtr *string
	if next != "" {
		nextPtr = &next
	}
	writeJSON(w, http.StatusOK, ListRunsResponse{Runs: summaries, NextCursor: nextPtr})
}

// AbortRun: POST /v1/runs/:id/abort — state transition only (worker propagation
// is engine work). 409 if already terminal, 404 if unknown.
func (s *Server) AbortRun(w http.ResponseWriter, r *http.Request) {
	id := chi.URLParam(r, "id")
	if _, err := uuid.Parse(id); err != nil {
		writeError(w, http.StatusNotFound, CodeNotFound, "run not found", "")
		return
	}
	abortedAt, err := s.Store.AbortRun(r.Context(), id)
	switch {
	case errors.Is(err, store.ErrNotFound):
		writeError(w, http.StatusNotFound, CodeNotFound, "run not found", "")
		return
	case errors.Is(err, store.ErrConflict):
		writeError(w, http.StatusConflict, CodeConflict, "run is already in a terminal state", "")
		return
	case err != nil:
		s.internal(w, "abort run", err)
		return
	}
	// Propagate the stop to the fleet so workers drain and quit (<1s). The DB
	// status is already aborted above; this is best-effort fleet teardown.
	s.abortOnCoordinator(id)

	s.Log.Info("run aborted", "run_id", id)
	writeJSON(w, http.StatusOK, AbortResponse{
		Status:    "aborted",
		AbortedAt: abortedAt.UTC().Format(time.RFC3339),
	})
}

// GetHistogram: GET /v1/runs/:id/histogram - api.md:148.
// ?format=json returns a binned JSON the web can render directly. Without
// format=json, returns the raw V2-deflate bytes with Content-Type
// application/hdr-v2+gzip so HDR-aware tooling can merge / re-percentile.
// ?which=uncorrected swaps to the uncorrected snapshot (defaults to corrected).
func (s *Server) GetHistogram(w http.ResponseWriter, r *http.Request) {
	if r.URL.Query().Get("format") == "json" {
		s.GetHistogramJSON(w, r)
		return
	}
	id := chi.URLParam(r, "id")
	if _, err := uuid.Parse(id); err != nil {
		writeError(w, http.StatusNotFound, CodeNotFound, "run not found", "")
		return
	}
	which := r.URL.Query().Get("which")
	if which == "" {
		which = "corrected"
	}
	var bytes []byte
	var err error
	switch which {
	case "corrected":
		bytes, err = s.Store.GetFinalHistogram(r.Context(), id)
	case "uncorrected":
		bytes, err = s.Store.GetFinalUncorrectedHistogram(r.Context(), id)
	default:
		writeError(w, http.StatusBadRequest, CodeInvalidRunSpec,
			"which must be corrected or uncorrected", "which")
		return
	}
	if errors.Is(err, store.ErrNotFound) {
		writeError(w, http.StatusNotFound, CodeNotFound, "run not found", "")
		return
	}
	if err != nil {
		s.internal(w, "get histogram", err)
		return
	}
	if len(bytes) == 0 {
		writeError(w, http.StatusConflict, CodeConflict,
			"histogram not yet finalized for this run", "")
		return
	}
	w.Header().Set("Content-Type", "application/hdr-v2+gzip")
	w.Header().Set("Content-Length", strconv.Itoa(len(bytes)))
	w.WriteHeader(http.StatusOK)
	_, _ = w.Write(bytes)
}

// Health: GET /healthz — checks DB connectivity.
func (s *Server) Health(w http.ResponseWriter, r *http.Request) {
	if err := s.Store.Ping(r.Context()); err != nil {
		writeError(w, http.StatusServiceUnavailable, CodeNoCapacity, "database unreachable", "")
		return
	}
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(http.StatusOK)
	_, _ = w.Write([]byte(`{"status":"ok"}`))
}

func (s *Server) internal(w http.ResponseWriter, what string, err error) {
	s.Log.Error("internal error", "op", what, "err", err.Error())
	writeError(w, http.StatusInternalServerError, CodeInternal, "internal error", "")
}

func runViewFrom(row *store.RunRow) RunView {
	v := RunView{
		RunID:             row.ID,
		Name:              derefStr(row.Name),
		Status:            row.Status,
		Target:            RunTargetView{URL: row.TargetURL, Method: row.TargetMethod},
		TargetRPS:         row.TargetRPS,
		DurationS:         row.DurationS,
		EffectiveRPS:      row.EffectiveRPS,
		CreatedAt:         row.CreatedAt.UTC().Format(time.RFC3339),
		CostPerRequestUSD: row.CostPerReqUSD,
	}
	if row.StartedAt != nil {
		s := row.StartedAt.UTC().Format(time.RFC3339)
		v.StartedAt = &s
	}
	if row.CompletedAt != nil {
		c := row.CompletedAt.UTC().Format(time.RFC3339)
		v.CompletedAt = &c
	}
	// elapsed_s only meaningful while running.
	if row.Status == "running" && row.StartedAt != nil {
		e := int(time.Since(*row.StartedAt).Seconds())
		v.ElapsedS = &e
	}
	// finals are only meaningful once an engine push has populated them.
	if row.P99US != nil && row.TotalRequests != nil {
		final := &FinalsView{
			Corrected: PercentilesView{
				P50US:  derefI64(row.P50US),
				P95US:  derefI64(row.P95US),
				P99US:  derefI64(row.P99US),
				P999US: derefI64(row.P999US),
			},
			TotalRequests: derefI64(row.TotalRequests),
			TotalPass:     derefI64(row.TotalPass),
		}
		if final.TotalRequests > 0 {
			final.CorrectnessPct = float64(final.TotalPass) / float64(final.TotalRequests) * 100.0
		}
		if row.CliffRPS != nil {
			final.CliffRPS = row.CliffRPS
		}
		v.Final = final
	}
	return v
}

func derefI64(p *int64) int64 {
	if p == nil {
		return 0
	}
	return *p
}

func rawOrEmpty(r json.RawMessage) string {
	if len(r) == 0 {
		return "{}"
	}
	return string(r)
}

func derefStr(p *string) string {
	if p == nil {
		return ""
	}
	return *p
}

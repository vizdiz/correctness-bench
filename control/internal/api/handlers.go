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
}

func NewServer(s *store.Store, log *slog.Logger) *Server {
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
	return &Server{Store: s, Log: log, Broker: sse.NewBroker(), Offload: cache}
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

	writeJSON(w, http.StatusCreated, CreateRunResponse{
		RunID:            id,
		Status:           "queued",
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
	writeJSON(w, http.StatusOK, runViewFrom(row))
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
	s.Log.Info("run aborted", "run_id", id)
	writeJSON(w, http.StatusOK, AbortResponse{
		Status:    "aborted",
		AbortedAt: abortedAt.UTC().Format(time.RFC3339),
	})
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
		RunID:        row.ID,
		Name:         derefStr(row.Name),
		Status:       row.Status,
		Target:       RunTargetView{URL: row.TargetURL, Method: row.TargetMethod},
		TargetRPS:    row.TargetRPS,
		DurationS:    row.DurationS,
		EffectiveRPS: row.EffectiveRPS,
		CreatedAt:    row.CreatedAt.UTC().Format(time.RFC3339),
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
	return v
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

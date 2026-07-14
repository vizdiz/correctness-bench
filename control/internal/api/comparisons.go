package api

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"io"
	"net/http"
	"time"

	"github.com/vizdiz/correctness-bench/control/internal/store"
)

// CompareCreateRequest fires two runs against two targets under ONE shared fleet
// epoch, so the comparison is fair: same time window, same conditions, each
// target at the full target_rps. Load shape is shared across both.
type CompareCreateRequest struct {
	Name              string          `json:"name,omitempty"`
	TargetA           Target          `json:"target_a"`
	TargetB           Target          `json:"target_b"`
	TargetRPS         float64         `json:"target_rps"`
	DurationS         int             `json:"duration_s"`
	WarmupS           int             `json:"warmup_s,omitempty"`
	LoadModel         string          `json:"load_model"`
	Connections       int             `json:"connections"`
	Keepalive         *bool           `json:"keepalive,omitempty"`
	Assert            json.RawMessage `json:"assert,omitempty"`
	RateLimitPolicy   json.RawMessage `json:"rate_limit_policy,omitempty"`
	EstimatedCostUSD  *float64        `json:"estimated_cost_usd,omitempty"`
	CostPerRequestUSD *float64        `json:"cost_per_request_usd,omitempty"`
}

// CompareCreateResponse identifies the concurrent pair.
type CompareCreateResponse struct {
	ComparisonID string `json:"comparison_id"`
	RunA         string `json:"run_a"`
	RunB         string `json:"run_b"`
}

// CreateComparison: POST /v1/comparisons - fair concurrent A/B under one epoch.
func (s *Server) CreateComparison(w http.ResponseWriter, r *http.Request) {
	var req CompareCreateRequest
	dec := json.NewDecoder(io.LimitReader(r.Body, maxBodyBytes))
	if err := dec.Decode(&req); err != nil {
		writeError(w, http.StatusBadRequest, CodeInvalidRunSpec, "request body is not valid JSON", "")
		return
	}
	if s.CoordinatorURL == "" {
		writeError(w, http.StatusServiceUnavailable, CodeNoCapacity, "dispatch is disabled; a comparison needs the coordinator", "")
		return
	}

	mk := func(name string, t Target) CreateRunRequest {
		return CreateRunRequest{
			Name: name, Target: t, TargetRPS: req.TargetRPS, DurationS: req.DurationS,
			WarmupS: req.WarmupS, LoadModel: req.LoadModel, Connections: req.Connections,
			Keepalive: req.Keepalive, Assert: req.Assert, RateLimitPolicy: req.RateLimitPolicy,
			EstimatedCostUSD: req.EstimatedCostUSD, CostPerRequestUSD: req.CostPerRequestUSD,
		}
	}
	base := req.Name
	if base == "" {
		base = "comparison"
	}
	reqA := mk(base+" - A", req.TargetA)
	reqB := mk(base+" - B", req.TargetB)
	for _, rr := range []*CreateRunRequest{&reqA, &reqB} {
		if ve := validateCreate(rr); ve != nil {
			writeError(w, http.StatusBadRequest, CodeInvalidRunSpec, ve.Message, ve.Field)
			return
		}
	}

	idA, err := s.insertRunRow(r.Context(), &reqA)
	if err != nil {
		s.internal(w, "insert comparison run a", err)
		return
	}
	idB, err := s.insertRunRow(r.Context(), &reqB)
	if err != nil {
		s.internal(w, "insert comparison run b", err)
		return
	}

	// One epoch, fired together: both fleets schedule against the same window.
	epoch := time.Now().UnixMicro()
	go s.dispatchToCoordinator(idA, &reqA, epoch)
	go s.dispatchToCoordinator(idB, &reqB, epoch)

	cmpID, err := s.Store.InsertComparison(r.Context(), base, []string{idA, idB})
	if err != nil {
		s.Log.Warn("insert comparison record", "err", err.Error())
	}
	s.Log.Info("comparison created", "comparison_id", cmpID, "run_a", idA, "run_b", idB, "epoch_us", epoch)
	writeJSON(w, http.StatusCreated, CompareCreateResponse{ComparisonID: cmpID, RunA: idA, RunB: idB})
}

// insertRunRow scrubs credentials and inserts a queued run, returning its id.
// Shared by the single-run and comparison paths. The request must already be
// validated. Target headers are scrubbed before anything is persisted.
func (s *Server) insertRunRow(ctx context.Context, req *CreateRunRequest) (string, error) {
	var bodySize *int
	if req.Target.BodyBase64 != "" {
		if raw, err := base64.StdEncoding.DecodeString(req.Target.BodyBase64); err == nil {
			n := len(raw)
			bodySize = &n
		}
	}
	scrubbed := ScrubHeaders(req.Target.Headers)
	headersJSON, err := json.Marshal(scrubbed)
	if err != nil {
		return "", err
	}
	var namePtr *string
	if req.Name != "" {
		namePtr = &req.Name
	}
	keepalive := true
	if req.Keepalive != nil {
		keepalive = *req.Keepalive
	}
	id, _, err := s.Store.InsertRun(ctx, store.InsertRunParams{
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
		Keepalive:           keepalive,
		AssertJSON:          rawOrEmpty(req.Assert),
		RateLimitJSON:       rawOrEmpty(req.RateLimitPolicy),
		EstimatedCostUSD:    req.EstimatedCostUSD,
		CostPerRequestUSD:   req.CostPerRequestUSD,
	})
	return id, err
}

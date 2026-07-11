package api

import (
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"

	"github.com/go-chi/chi/v5"
	"github.com/google/uuid"

	"github.com/vizdiz/correctness-bench/control/internal/store"
)

// RegressionRequest is the body of POST /v1/runs/:id/regression-check.
type RegressionRequest struct {
	BaselineRunID string                `json:"baseline_run_id"`
	Thresholds    RegressionThresholds  `json:"thresholds"`
}

// RegressionThresholds are percentage points. p99_delta_pct > threshold means
// fail. correctness_delta_pct: negative means worse; we fail when the magnitude
// of the negative delta exceeds the threshold.
type RegressionThresholds struct {
	P99DeltaPct         float64 `json:"p99_delta_pct"`
	CorrectnessDeltaPct float64 `json:"correctness_delta_pct"`
}

// RegressionResponse mirrors contracts/api.md:228.
type RegressionResponse struct {
	Passed              bool    `json:"passed"`
	P99DeltaPct         float64 `json:"p99_delta_pct"`
	CorrectnessDeltaPct float64 `json:"correctness_delta_pct"`
	CliffRPSDelta       float64 `json:"cliff_rps_delta"`
	Details             string  `json:"details"`
}

// RegressionCheck: POST /v1/runs/:id/regression-check - api.md:228.
func (s *Server) RegressionCheck(w http.ResponseWriter, r *http.Request) {
	candidateID := chi.URLParam(r, "id")
	if _, err := uuid.Parse(candidateID); err != nil {
		writeError(w, http.StatusNotFound, CodeNotFound, "run not found", "")
		return
	}

	var req RegressionRequest
	if err := json.NewDecoder(io.LimitReader(r.Body, 1<<14)).Decode(&req); err != nil {
		writeError(w, http.StatusBadRequest, CodeInvalidRunSpec, "request body is not valid JSON", "")
		return
	}
	if _, err := uuid.Parse(req.BaselineRunID); err != nil {
		writeError(w, http.StatusBadRequest, CodeInvalidRunSpec, "baseline_run_id is not a uuid", "baseline_run_id")
		return
	}
	if req.Thresholds.P99DeltaPct <= 0 || req.Thresholds.CorrectnessDeltaPct <= 0 {
		writeError(w, http.StatusBadRequest, CodeInvalidRunSpec,
			"thresholds.p99_delta_pct and thresholds.correctness_delta_pct must be > 0",
			"thresholds")
		return
	}

	cand, err := s.Store.GetRun(r.Context(), candidateID)
	if errors.Is(err, store.ErrNotFound) {
		writeError(w, http.StatusNotFound, CodeNotFound, "candidate run not found", "")
		return
	}
	if err != nil {
		s.internal(w, "get candidate", err)
		return
	}
	base, err := s.Store.GetRun(r.Context(), req.BaselineRunID)
	if errors.Is(err, store.ErrNotFound) {
		writeError(w, http.StatusNotFound, CodeNotFound, "baseline run not found", "")
		return
	}
	if err != nil {
		s.internal(w, "get baseline", err)
		return
	}

	// Both must be finalized to compare.
	if cand.P99US == nil || base.P99US == nil {
		writeError(w, http.StatusConflict, CodeConflict,
			"both runs must be finalized before regression-check", "")
		return
	}

	p99Delta := pctDelta(float64(*base.P99US), float64(*cand.P99US))
	correctnessDelta := correctnessPctDelta(base, cand)
	cliffDelta := cliffRPSDelta(base, cand)

	failures := []string{}
	if p99Delta > req.Thresholds.P99DeltaPct {
		failures = append(failures, fmt.Sprintf(
			"p99 regressed by %.1f%% (threshold %.1f%%)",
			p99Delta, req.Thresholds.P99DeltaPct,
		))
	}
	if -correctnessDelta > req.Thresholds.CorrectnessDeltaPct {
		failures = append(failures, fmt.Sprintf(
			"correctness dropped by %.1f%% (threshold %.1f%%)",
			-correctnessDelta, req.Thresholds.CorrectnessDeltaPct,
		))
	}
	resp := RegressionResponse{
		Passed:              len(failures) == 0,
		P99DeltaPct:         p99Delta,
		CorrectnessDeltaPct: correctnessDelta,
		CliffRPSDelta:       cliffDelta,
	}
	if len(failures) == 0 {
		resp.Details = "within thresholds"
	} else {
		resp.Details = failures[0]
		for _, f := range failures[1:] {
			resp.Details += "; " + f
		}
	}
	writeJSON(w, http.StatusOK, resp)
}

// pctDelta returns (candidate - baseline) / baseline * 100. A positive value
// means the candidate is HIGHER than baseline (for p99: worse).
func pctDelta(baseline, candidate float64) float64 {
	if baseline == 0 {
		return 0
	}
	return (candidate - baseline) / baseline * 100.0
}

// correctnessPctDelta is signed: negative means the candidate had worse pass
// rate. Returned in absolute percentage points (not relative %), e.g. 99% -> 96%
// is -3.0.
func correctnessPctDelta(base, cand *store.RunRow) float64 {
	bc := correctnessFraction(base)
	cc := correctnessFraction(cand)
	if bc < 0 || cc < 0 {
		return 0
	}
	return (cc - bc) * 100.0
}

func correctnessFraction(r *store.RunRow) float64 {
	if r.TotalRequests == nil || r.TotalPass == nil || *r.TotalRequests == 0 {
		return -1
	}
	return float64(*r.TotalPass) / float64(*r.TotalRequests)
}

func cliffRPSDelta(base, cand *store.RunRow) float64 {
	if base.CliffRPS == nil || cand.CliffRPS == nil {
		return 0
	}
	return *cand.CliffRPS - *base.CliffRPS
}

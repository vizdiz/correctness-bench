package api

import (
	"errors"
	"net/http"

	"github.com/go-chi/chi/v5"
	"github.com/google/uuid"

	"github.com/vizdiz/correctness-bench/control/internal/store"
)

// CompareResponse mirrors contracts/api.md:204 for GET /v1/runs/:id/compare/:id2.
type CompareResponse struct {
	A         RunView    `json:"a"`
	B         RunView    `json:"b"`
	WinnerBy  WinnerBy   `json:"winner_by"`
	Fairness  Fairness   `json:"fairness"`
}

// WinnerBy names the side ("a" | "b" | "" for tie / unknown) that wins each
// axis. Cost-per-request needs both runs to have declared cost_per_request_usd;
// rate-limit onset needs both finalized with values; empty axis = N/A.
type WinnerBy struct {
	LatencyP99      string `json:"latency_p99"`
	Correctness     string `json:"correctness"`
	CostPerRequest  string `json:"cost_per_request"`
	TailStability   string `json:"tail_stability"`
	RateLimitOnset  string `json:"rate_limit_onset"`
}

// Fairness exposes whether the comparison was apples-to-apples. The UI surfaces
// a banner if any flag is false.
type Fairness struct {
	Interleaved    bool `json:"interleaved"`
	SameAssertSpec bool `json:"same_assert_spec"`
	SameLoadShape  bool `json:"same_load_shape"`
}

// Compare: GET /v1/runs/:id/compare/:id2 - api.md:204.
func (s *Server) Compare(w http.ResponseWriter, r *http.Request) {
	idA := chi.URLParam(r, "id")
	idB := chi.URLParam(r, "id2")
	if _, err := uuid.Parse(idA); err != nil {
		writeError(w, http.StatusNotFound, CodeNotFound, "run not found", "")
		return
	}
	if _, err := uuid.Parse(idB); err != nil {
		writeError(w, http.StatusNotFound, CodeNotFound, "run not found", "")
		return
	}

	rowA, err := s.Store.GetRun(r.Context(), idA)
	if errors.Is(err, store.ErrNotFound) {
		writeError(w, http.StatusNotFound, CodeNotFound, "run a not found", "")
		return
	}
	if err != nil {
		s.internal(w, "get run a", err)
		return
	}
	rowB, err := s.Store.GetRun(r.Context(), idB)
	if errors.Is(err, store.ErrNotFound) {
		writeError(w, http.StatusNotFound, CodeNotFound, "run b not found", "")
		return
	}
	if err != nil {
		s.internal(w, "get run b", err)
		return
	}

	a := runViewFrom(rowA)
	b := runViewFrom(rowB)
	// Attach offload counts on both sides so the UI doesn't have to call again.
	if c, err := s.Store.OffloadCountsForRun(r.Context(), idA); err == nil &&
		c.Pass+c.FailSchema+c.FailValue+c.FailRegex > 0 {
		a.Offload = &OffloadCountsView{Pass: c.Pass, FailSchema: c.FailSchema, FailValue: c.FailValue, FailRegex: c.FailRegex}
	}
	if c, err := s.Store.OffloadCountsForRun(r.Context(), idB); err == nil &&
		c.Pass+c.FailSchema+c.FailValue+c.FailRegex > 0 {
		b.Offload = &OffloadCountsView{Pass: c.Pass, FailSchema: c.FailSchema, FailValue: c.FailValue, FailRegex: c.FailRegex}
	}

	// Fair when the two runs were fired together under one epoch.
	concurrent, _ := s.Store.RunsWereComparedTogether(r.Context(), idA, idB)
	resp := CompareResponse{
		A:        a,
		B:        b,
		WinnerBy: pickWinners(rowA, rowB),
		Fairness: pickFairness(rowA, rowB, concurrent),
	}
	writeJSON(w, http.StatusOK, resp)
}

// pickWinners decides each axis by "lower is better" or "higher is better"
// depending on the metric. Empty string means insufficient data on at least
// one side to judge.
func pickWinners(a, b *store.RunRow) WinnerBy {
	lower := func(x, y *int64) string {
		if x == nil || y == nil {
			return ""
		}
		if *x < *y {
			return "a"
		}
		if *x > *y {
			return "b"
		}
		return ""
	}
	lowerF := func(x, y *float64) string {
		if x == nil || y == nil {
			return ""
		}
		if *x < *y {
			return "a"
		}
		if *x > *y {
			return "b"
		}
		return ""
	}
	higherF := func(x, y *float64) string {
		if x == nil || y == nil {
			return ""
		}
		if *x > *y {
			return "a"
		}
		if *x < *y {
			return "b"
		}
		return ""
	}

	correctness := func(r *store.RunRow) *float64 {
		if r.TotalRequests == nil || r.TotalPass == nil || *r.TotalRequests == 0 {
			return nil
		}
		v := float64(*r.TotalPass) / float64(*r.TotalRequests)
		return &v
	}
	// tail_stability: lower (p99 - p50) is better - tighter tail.
	tail := func(r *store.RunRow) *int64 {
		if r.P50US == nil || r.P99US == nil {
			return nil
		}
		v := *r.P99US - *r.P50US
		return &v
	}

	return WinnerBy{
		LatencyP99:     lower(a.P99US, b.P99US),
		Correctness:    higherF(correctness(a), correctness(b)),
		CostPerRequest: lowerF(a.CostPerReqUSD, b.CostPerReqUSD),
		TailStability:  lower(tail(a), tail(b)),
		RateLimitOnset: higherF(a.CliffRPS, b.CliffRPS),
	}
}

func pickFairness(a, b *store.RunRow, concurrent bool) Fairness {
	return Fairness{
		// True when the two runs were fired together under one shared epoch
		// (concurrent A/B) - no sequential time confound. False for a post-hoc
		// join of two independently-run rows.
		Interleaved:    concurrent,
		SameAssertSpec: false, // assert_spec equality check is out of scope until we read it
		SameLoadShape:  a.TargetRPS == b.TargetRPS && a.DurationS == b.DurationS,
	}
}

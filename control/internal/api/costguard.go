package api

import (
	"context"
	"fmt"
	"sync"
	"time"
)

// costState is a run's in-flight cost-ceiling state, cached after the first
// tick so we don't read the DB every second. Gating is on only when the user
// declared both cost_per_request_usd and estimated_cost_usd.
type costState struct {
	mu      sync.Mutex
	active  bool
	perReq  float64
	ceiling float64 // estimated_cost_usd * 1.1
	tripped bool
}

// checkCost enforces the api.md cost ceiling on an in-flight run: once cumulative
// requests * cost_per_request exceed estimated_cost * 1.1, abort the run. Called
// per tick (async) with the cumulative completed count. Best-effort and idempotent.
func (s *Server) checkCost(runID string, completedTotal uint64) {
	v, ok := s.costGuard.Load(runID)
	if !ok {
		st := &costState{}
		ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
		perReq, est, err := s.Store.GetRunCost(ctx, runID)
		cancel()
		if err == nil && perReq != nil && *perReq > 0 && est != nil {
			st.active = true
			st.perReq = *perReq
			st.ceiling = *est * 1.1
		}
		actual, _ := s.costGuard.LoadOrStore(runID, st)
		v = actual
	}
	st := v.(*costState)
	if !st.active {
		return
	}
	st.mu.Lock()
	defer st.mu.Unlock()
	if st.tripped {
		return
	}
	cost := float64(completedTotal) * st.perReq
	if cost <= st.ceiling {
		return
	}
	st.tripped = true
	reason := fmt.Sprintf("cost ceiling exceeded: %d requests x $%.6f = $%.2f > $%.2f",
		completedTotal, st.perReq, cost, st.ceiling)
	s.Log.Warn("cost auto-abort", "run_id", runID, "reason", reason)
	if err := s.Store.MarkAborted(context.Background(), runID, reason); err != nil {
		s.Log.Warn("cost abort mark", "run_id", runID, "err", err.Error())
	}
	// Stop the fleet (reuses the abort propagation path).
	go s.abortOnCoordinator(runID)
}

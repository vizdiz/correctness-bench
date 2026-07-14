package api

import "encoding/json"

// Target is the request-side target spec. Headers are WRITE-ONLY: accepted on
// POST, scrubbed before persistence, and NEVER present on any response type.
type Target struct {
	URL        string            `json:"url"`
	Method     string            `json:"method"`
	Headers    map[string]string `json:"headers,omitempty"`
	BodyBase64 string            `json:"body_base64,omitempty"`
	TimeoutMS  int               `json:"timeout_ms,omitempty"`
	VerifyTLS  *bool             `json:"verify_tls,omitempty"`
}

// CreateRunRequest is the POST /v1/runs body. Pointer fields distinguish
// "absent" from "zero" where the contract has defaults.
type CreateRunRequest struct {
	Name              string          `json:"name,omitempty"`
	Target            Target          `json:"target"`
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

// CreateRunResponse is the 201 body.
type CreateRunResponse struct {
	RunID            string   `json:"run_id"`
	Status           string   `json:"status"`
	EstimatedCostUSD *float64 `json:"estimated_cost_usd,omitempty"`
}

// RunTargetView is the only target shape ever returned: url + method, NO headers.
type RunTargetView struct {
	URL    string `json:"url"`
	Method string `json:"method"`
}

// OffloadCountsView is the per-verdict roll-up exposed alongside inline
// fail-by-tier counts. Omitted entirely when no offload tier was configured
// for the run (i.e., all four are zero and no rows exist).
type OffloadCountsView struct {
	Pass       int64 `json:"pass"`
	FailSchema int64 `json:"fail_schema"`
	FailValue  int64 `json:"fail_value"`
	FailRegex  int64 `json:"fail_regex"`
}

// PercentilesView is the corrected-latency final percentiles. All zero when
// the run hasn't been finalized.
type PercentilesView struct {
	P50US  int64 `json:"p50_us"`
	P95US  int64 `json:"p95_us"`
	P99US  int64 `json:"p99_us"`
	P999US int64 `json:"p999_us,omitempty"`
}

// FinalsView holds the per-run final aggregates surfaced once the engine
// reports a `finalize`. Nil on running / queued / failed-without-finalize.
type FinalsView struct {
	Corrected      PercentilesView `json:"corrected"`
	TotalRequests  int64           `json:"total_requests"`
	TotalPass      int64           `json:"total_pass"`
	CorrectnessPct float64         `json:"correctness_pct"`
	CliffRPS       *float64        `json:"cliff_rps,omitempty"`
}

// RunView is GET /v1/runs/:id. Fields that don't apply to the current status are
// omitted. There is deliberately no Headers field anywhere in this type.
type RunView struct {
	RunID        string             `json:"run_id"`
	Name         string             `json:"name,omitempty"`
	Status       string             `json:"status"`
	Target       RunTargetView      `json:"target"`
	TargetRPS    float64            `json:"target_rps"`
	DurationS    int                `json:"duration_s,omitempty"`
	EffectiveRPS *float64           `json:"effective_rps,omitempty"`
	CreatedAt    string             `json:"created_at,omitempty"`
	StartedAt    *string            `json:"started_at,omitempty"`
	CompletedAt  *string            `json:"completed_at,omitempty"`
	ElapsedS     *int               `json:"elapsed_s,omitempty"`
	Offload           *OffloadCountsView `json:"offload,omitempty"`
	Final             *FinalsView        `json:"final,omitempty"`
	CostPerRequestUSD *float64           `json:"cost_per_request_usd,omitempty"`
	StatusReason      string             `json:"status_reason,omitempty"`
	Warnings          json.RawMessage    `json:"warnings,omitempty"`
}

// RunSummary is a compact list-row.
type RunSummary struct {
	RunID     string  `json:"run_id"`
	Name      string  `json:"name,omitempty"`
	Status    string  `json:"status"`
	TargetRPS float64 `json:"target_rps"`
	DurationS int     `json:"duration_s"`
	CreatedAt string  `json:"created_at"`
}

// ListRunsResponse is GET /v1/runs. next_cursor is null on the last page.
type ListRunsResponse struct {
	Runs       []RunSummary `json:"runs"`
	NextCursor *string      `json:"next_cursor"`
}

// AbortResponse is POST /v1/runs/:id/abort.
type AbortResponse struct {
	Status    string `json:"status"`
	AbortedAt string `json:"aborted_at"`
}

package api

import (
	"encoding/json"
	"errors"
	"io"
	"net/http"
	"time"

	"github.com/go-chi/chi/v5"
	"github.com/google/uuid"

	"github.com/vizdiz/correctness-bench/control/internal/store"
)

// CreateTemplateRequest mirrors CreateRunRequest plus a required template name.
// On persistence, target.headers values are passed through ScrubHeaders so the
// stored spec never carries secrets.
type CreateTemplateRequest struct {
	Name string          `json:"name"`
	Spec CreateRunRequest `json:"spec"`
}

// TemplateView is the read-side shape.
type TemplateView struct {
	ID           string           `json:"id"`
	Name         string           `json:"name"`
	Spec         CreateRunRequest `json:"spec"`
	CreatedAt    string           `json:"created_at"`
	LastUsedAt   *string          `json:"last_used_at,omitempty"`
}

// ListTemplatesResponse is GET /v1/templates.
type ListTemplatesResponse struct {
	Templates []TemplateView `json:"templates"`
}

// RunFromTemplateRequest re-supplies the redacted secrets on POST /v1/templates/:id/run.
// Headers map merges INTO the stored target.headers; any key present here
// replaces the "***" placeholder. Other run-spec fields (target_rps, duration_s, etc.)
// fall back to the template's stored values.
type RunFromTemplateRequest struct {
	Name              string            `json:"name,omitempty"`
	Headers           map[string]string `json:"headers,omitempty"`
	TargetRPS         *float64          `json:"target_rps,omitempty"`
	DurationS         *int              `json:"duration_s,omitempty"`
	EstimatedCostUSD  *float64          `json:"estimated_cost_usd,omitempty"`
	CostPerRequestUSD *float64          `json:"cost_per_request_usd,omitempty"`
}

// CreateTemplate: POST /v1/templates - api.md:249.
func (s *Server) CreateTemplate(w http.ResponseWriter, r *http.Request) {
	var req CreateTemplateRequest
	if err := json.NewDecoder(io.LimitReader(r.Body, maxBodyBytes)).Decode(&req); err != nil {
		writeError(w, http.StatusBadRequest, CodeInvalidRunSpec, "request body is not valid JSON", "")
		return
	}
	if req.Name == "" {
		writeError(w, http.StatusBadRequest, CodeInvalidRunSpec, "name is required", "name")
		return
	}
	if ve := validateCreate(&req.Spec); ve != nil {
		writeError(w, http.StatusBadRequest, CodeInvalidRunSpec, ve.Message, ve.Field)
		return
	}

	// Always scrub before persisting - the contract guarantees no secrets in
	// templates regardless of what the client sent.
	req.Spec.Target.Headers = ScrubHeaders(req.Spec.Target.Headers)
	specJSON, err := json.Marshal(req.Spec)
	if err != nil {
		s.internal(w, "marshal template spec", err)
		return
	}

	id, createdAt, err := s.Store.InsertTemplate(r.Context(), req.Name, specJSON)
	if err != nil {
		s.internal(w, "insert template", err)
		return
	}
	s.Log.Info("template created", "template_id", id, "name", req.Name)

	writeJSON(w, http.StatusCreated, TemplateView{
		ID:        id,
		Name:      req.Name,
		Spec:      req.Spec,
		CreatedAt: createdAt.UTC().Format(time.RFC3339),
	})
}

// ListTemplates: GET /v1/templates - api.md:250.
func (s *Server) ListTemplates(w http.ResponseWriter, r *http.Request) {
	rows, err := s.Store.ListTemplates(r.Context(), 50)
	if err != nil {
		s.internal(w, "list templates", err)
		return
	}
	out := make([]TemplateView, 0, len(rows))
	for _, row := range rows {
		var spec CreateRunRequest
		if err := json.Unmarshal(row.SpecRedacted, &spec); err != nil {
			s.Log.Warn("template spec decode", "template_id", row.ID, "err", err.Error())
			continue
		}
		v := TemplateView{
			ID:        row.ID,
			Name:      row.Name,
			Spec:      spec,
			CreatedAt: row.CreatedAt.UTC().Format(time.RFC3339),
		}
		if row.LastUsedAt != nil {
			u := row.LastUsedAt.UTC().Format(time.RFC3339)
			v.LastUsedAt = &u
		}
		out = append(out, v)
	}
	writeJSON(w, http.StatusOK, ListTemplatesResponse{Templates: out})
}

// RunFromTemplate: POST /v1/templates/:id/run - api.md:251.
// Forks the stored template into a fresh run row, merging in re-supplied
// secret values, then enqueues the run (status=queued). The actual engine
// dispatch is external; the response is the new run_id, matching POST /v1/runs.
func (s *Server) RunFromTemplate(w http.ResponseWriter, r *http.Request) {
	id := chi.URLParam(r, "id")
	if _, err := uuid.Parse(id); err != nil {
		writeError(w, http.StatusNotFound, CodeNotFound, "template not found", "")
		return
	}
	var req RunFromTemplateRequest
	if r.ContentLength > 0 {
		if err := json.NewDecoder(io.LimitReader(r.Body, maxBodyBytes)).Decode(&req); err != nil {
			writeError(w, http.StatusBadRequest, CodeInvalidRunSpec, "request body is not valid JSON", "")
			return
		}
	}

	row, err := s.Store.GetTemplate(r.Context(), id)
	if errors.Is(err, store.ErrNotFound) {
		writeError(w, http.StatusNotFound, CodeNotFound, "template not found", "")
		return
	}
	if err != nil {
		s.internal(w, "get template", err)
		return
	}

	var spec CreateRunRequest
	if err := json.Unmarshal(row.SpecRedacted, &spec); err != nil {
		s.internal(w, "decode template spec", err)
		return
	}

	// Merge re-supplied secrets into the template's redacted headers. Any
	// header value still equal to "***" after the merge is rejected so we
	// never fire a request the user thought was authenticated.
	if spec.Target.Headers == nil {
		spec.Target.Headers = map[string]string{}
	}
	for k, v := range req.Headers {
		spec.Target.Headers[k] = v
	}
	for k, v := range spec.Target.Headers {
		if v == "***" {
			writeError(w, http.StatusBadRequest, CodeInvalidRunSpec,
				"header value still redacted; re-supply on /run", "headers."+k)
			return
		}
	}

	// Optional overrides from the request body.
	if req.Name != "" {
		spec.Name = req.Name
	}
	if req.TargetRPS != nil {
		spec.TargetRPS = *req.TargetRPS
	}
	if req.DurationS != nil {
		spec.DurationS = *req.DurationS
	}
	if req.EstimatedCostUSD != nil {
		spec.EstimatedCostUSD = req.EstimatedCostUSD
	}
	if req.CostPerRequestUSD != nil {
		spec.CostPerRequestUSD = req.CostPerRequestUSD
	}

	if ve := validateCreate(&spec); ve != nil {
		writeError(w, http.StatusBadRequest, CodeInvalidRunSpec, ve.Message, ve.Field)
		return
	}

	// Persist the actual run. Re-uses CreateRun's plumbing path so secrets
	// are scrubbed again at run-storage time even if the caller skipped them.
	scrubbed := ScrubHeaders(spec.Target.Headers)
	headersJSON, err := json.Marshal(scrubbed)
	if err != nil {
		s.internal(w, "marshal headers", err)
		return
	}
	var namePtr *string
	if spec.Name != "" {
		namePtr = &spec.Name
	}
	runID, _, err := s.Store.InsertRun(r.Context(), store.InsertRunParams{
		Name:                namePtr,
		TargetURL:           spec.Target.URL,
		TargetMethod:        spec.Target.Method,
		HeadersRedactedJSON: string(headersJSON),
		BodySize:            nil,
		TargetRPS:           spec.TargetRPS,
		DurationS:           spec.DurationS,
		WarmupS:             spec.WarmupS,
		LoadModel:           spec.LoadModel,
		Connections:         spec.Connections,
		Keepalive:           *spec.Keepalive,
		AssertJSON:          rawOrEmpty(spec.Assert),
		RateLimitJSON:       rawOrEmpty(spec.RateLimitPolicy),
		EstimatedCostUSD:    spec.EstimatedCostUSD,
		CostPerRequestUSD:   spec.CostPerRequestUSD,
	})
	if err != nil {
		s.internal(w, "insert run from template", err)
		return
	}
	if err := s.Store.MarkTemplateUsed(r.Context(), id); err != nil &&
		!errors.Is(err, store.ErrNotFound) {
		s.Log.Warn("mark template used", "template_id", id, "err", err.Error())
	}

	writeJSON(w, http.StatusCreated, CreateRunResponse{
		RunID:            runID,
		Status:           "queued",
		EstimatedCostUSD: spec.EstimatedCostUSD,
	})
}

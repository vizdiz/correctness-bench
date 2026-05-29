package api

import (
	"fmt"
	"net/url"
	"strings"
)

type validationError struct {
	Field   string
	Message string
}

func (e *validationError) Error() string { return e.Message }

var allowedMethods = map[string]struct{}{
	"GET": {}, "POST": {}, "PUT": {}, "PATCH": {}, "DELETE": {}, "HEAD": {},
}

var allowedLoadModels = map[string]struct{}{"open": {}, "closed": {}}

// validateCreate checks a run spec and normalizes it in place (method upper-cased,
// keepalive defaulted). Returns a *validationError (with the offending field) on
// the first problem, or nil if the spec is acceptable.
func validateCreate(r *CreateRunRequest) *validationError {
	if strings.TrimSpace(r.Target.URL) == "" {
		return &validationError{"target.url", "target.url is required"}
	}
	u, err := url.Parse(r.Target.URL)
	if err != nil || (u.Scheme != "http" && u.Scheme != "https") || u.Host == "" {
		return &validationError{"target.url", "target.url must be a valid http(s) URL"}
	}

	r.Target.Method = strings.ToUpper(strings.TrimSpace(r.Target.Method))
	if r.Target.Method == "" {
		return &validationError{"target.method", "target.method is required"}
	}
	if _, ok := allowedMethods[r.Target.Method]; !ok {
		return &validationError{"target.method", "target.method must be one of GET,POST,PUT,PATCH,DELETE,HEAD"}
	}

	if r.TargetRPS <= 0 {
		return &validationError{"target_rps", "target_rps must be > 0"}
	}
	if r.DurationS <= 0 {
		return &validationError{"duration_s", "duration_s must be > 0"}
	}
	if r.WarmupS < 0 {
		return &validationError{"warmup_s", "warmup_s must be >= 0"}
	}
	if _, ok := allowedLoadModels[r.LoadModel]; !ok {
		return &validationError{"load_model", "load_model must be 'open' or 'closed'"}
	}
	if r.Connections <= 0 {
		return &validationError{"connections", "connections must be > 0"}
	}
	if r.Keepalive == nil {
		def := true
		r.Keepalive = &def
	}

	if ve := validateCostCeiling(r); ve != nil {
		return ve
	}
	return nil
}

// validateCostCeiling enforces the api.md rule: when cost_per_request_usd is set,
// reject runs whose projected cost exceeds estimated_cost_usd * 1.1.
func validateCostCeiling(r *CreateRunRequest) *validationError {
	if r.CostPerRequestUSD == nil {
		return nil // cost gating off
	}
	if *r.CostPerRequestUSD < 0 {
		return &validationError{"cost_per_request_usd", "cost_per_request_usd must be >= 0"}
	}
	if r.EstimatedCostUSD == nil {
		return &validationError{"estimated_cost_usd", "estimated_cost_usd is required when cost_per_request_usd is set"}
	}
	projected := r.TargetRPS * float64(r.DurationS) * *r.CostPerRequestUSD
	ceiling := *r.EstimatedCostUSD * 1.1
	if projected > ceiling {
		return &validationError{
			"estimated_cost_usd",
			fmt.Sprintf("projected cost $%.4f exceeds ceiling $%.4f (estimated_cost_usd * 1.1)", projected, ceiling),
		}
	}
	return nil
}

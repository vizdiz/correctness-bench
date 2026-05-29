package api

import "testing"

func validReq() *CreateRunRequest {
	ka := true
	return &CreateRunRequest{
		Target:      Target{URL: "https://api.example.com/v1/foo", Method: "post"},
		TargetRPS:   100,
		DurationS:   60,
		LoadModel:   "open",
		Connections: 50,
		Keepalive:   &ka,
	}
}

func TestValidate_OK_NormalizesMethodAndDefaultsKeepalive(t *testing.T) {
	r := validReq()
	r.Keepalive = nil // exercise default
	if ve := validateCreate(r); ve != nil {
		t.Fatalf("expected valid, got %v", ve)
	}
	if r.Target.Method != "POST" {
		t.Errorf("method should be upper-cased, got %q", r.Target.Method)
	}
	if r.Keepalive == nil || *r.Keepalive != true {
		t.Errorf("keepalive should default to true")
	}
}

func TestValidate_RejectsBadFields(t *testing.T) {
	cases := []struct {
		name  string
		mut   func(*CreateRunRequest)
		field string
	}{
		{"empty url", func(r *CreateRunRequest) { r.Target.URL = "" }, "target.url"},
		{"non-http url", func(r *CreateRunRequest) { r.Target.URL = "ftp://x/y" }, "target.url"},
		{"bad method", func(r *CreateRunRequest) { r.Target.Method = "TRACE" }, "target.method"},
		{"zero rps", func(r *CreateRunRequest) { r.TargetRPS = 0 }, "target_rps"},
		{"zero duration", func(r *CreateRunRequest) { r.DurationS = 0 }, "duration_s"},
		{"bad load model", func(r *CreateRunRequest) { r.LoadModel = "burst" }, "load_model"},
		{"zero connections", func(r *CreateRunRequest) { r.Connections = 0 }, "connections"},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			r := validReq()
			c.mut(r)
			ve := validateCreate(r)
			if ve == nil {
				t.Fatalf("expected validation error for %s", c.name)
			}
			if ve.Field != c.field {
				t.Errorf("expected field %q, got %q (%s)", c.field, ve.Field, ve.Message)
			}
		})
	}
}

func TestValidate_CostCeiling(t *testing.T) {
	cpr := 0.001
	t.Run("within ceiling passes", func(t *testing.T) {
		r := validReq() // 100 rps * 60s * 0.001 = $6.00
		est := 6.0      // ceiling = 6.6
		r.CostPerRequestUSD = &cpr
		r.EstimatedCostUSD = &est
		if ve := validateCreate(r); ve != nil {
			t.Fatalf("expected pass within ceiling, got %v", ve)
		}
	})
	t.Run("over ceiling rejected", func(t *testing.T) {
		r := validReq() // projected $6.00
		est := 5.0      // ceiling = 5.5 < 6.0 -> reject
		r.CostPerRequestUSD = &cpr
		r.EstimatedCostUSD = &est
		ve := validateCreate(r)
		if ve == nil || ve.Field != "estimated_cost_usd" {
			t.Fatalf("expected cost-ceiling rejection, got %v", ve)
		}
	})
	t.Run("cost_per_request without estimate rejected", func(t *testing.T) {
		r := validReq()
		r.CostPerRequestUSD = &cpr
		ve := validateCreate(r)
		if ve == nil || ve.Field != "estimated_cost_usd" {
			t.Fatalf("expected required-estimate rejection, got %v", ve)
		}
	})
	t.Run("no cost_per_request means gating off", func(t *testing.T) {
		r := validReq()
		if ve := validateCreate(r); ve != nil {
			t.Fatalf("expected pass with gating off, got %v", ve)
		}
	})
}

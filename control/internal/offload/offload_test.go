package offload

import (
	"encoding/json"
	"testing"
)

func TestEvaluator_PathTier_DetectsWrongValue(t *testing.T) {
	// Mock's healthy body: {"status":"ok","count":3,"items":["a","b","c"]}
	// Mock's wrong_value body: count is -1 instead of 3.
	healthy := []byte(`{"status":"ok","count":3,"items":["a","b","c"]}`)
	wrong := []byte(`{"status":"ok","count":-1,"items":["a","b","c"]}`)

	spec := Spec{
		Paths: []PathAssertion{
			{Path: "status", Equals: "ok"},
			{Path: "count", Equals: float64(3)},
		},
	}
	e, err := NewEvaluator(spec)
	if err != nil {
		t.Fatalf("NewEvaluator: %v", err)
	}

	if v, _ := e.Evaluate(healthy); v != VerdictPass {
		t.Errorf("expected pass on healthy, got %s", v)
	}
	v, details := e.Evaluate(wrong)
	if v != VerdictFailValue {
		t.Errorf("expected fail_value on wrong_value body, got %s", v)
	}
	if details == "" {
		t.Errorf("expected non-empty details on fail")
	}
}

func TestEvaluator_PathTier_DetectsMissingField(t *testing.T) {
	spec := Spec{Paths: []PathAssertion{{Path: "missing"}}} // no Equals - existence only
	e, _ := NewEvaluator(spec)
	if v, _ := e.Evaluate([]byte(`{"other":1}`)); v != VerdictFailValue {
		t.Errorf("missing path should be fail_value, got %s", v)
	}
	if v, _ := e.Evaluate([]byte(`{"missing":null}`)); v != VerdictPass {
		t.Errorf("present-but-null should pass when no Equals, got %s", v)
	}
}

func TestEvaluator_RegexTier(t *testing.T) {
	spec := Spec{Patterns: []string{`"status":\s*"ok"`}}
	e, err := NewEvaluator(spec)
	if err != nil {
		t.Fatalf("NewEvaluator: %v", err)
	}
	if v, _ := e.Evaluate([]byte(`{"status":"ok"}`)); v != VerdictPass {
		t.Errorf("matching body should pass, got %s", v)
	}
	if v, _ := e.Evaluate([]byte(`{"status":"err"}`)); v != VerdictFailRegex {
		t.Errorf("non-matching body should be fail_regex, got %s", v)
	}
}

func TestEvaluator_NoRules_AllPass(t *testing.T) {
	e, _ := NewEvaluator(Spec{})
	if v, _ := e.Evaluate([]byte(`anything`)); v != VerdictPass {
		t.Errorf("empty spec should pass everything, got %s", v)
	}
}

func TestEvaluator_BadRegex_ReportsError(t *testing.T) {
	_, err := NewEvaluator(Spec{Patterns: []string{"["}})
	if err == nil {
		t.Errorf("invalid regex should fail to compile")
	}
}

func TestParseSpecFromAssert(t *testing.T) {
	raw := json.RawMessage(`{
		"expected_status": [200],
		"max_latency_us": 200000,
		"paths": [{"path": "count", "equals": 3}],
		"patterns": ["status"]
	}`)
	spec, err := ParseSpecFromAssert(raw)
	if err != nil {
		t.Fatalf("ParseSpecFromAssert: %v", err)
	}
	if len(spec.Paths) != 1 || spec.Paths[0].Path != "count" {
		t.Errorf("bad paths: %+v", spec.Paths)
	}
	if len(spec.Patterns) != 1 {
		t.Errorf("bad patterns: %v", spec.Patterns)
	}
	if !spec.HasAnyRule() {
		t.Errorf("HasAnyRule should be true")
	}
}

func TestParseSpecFromAssert_Empty(t *testing.T) {
	spec, _ := ParseSpecFromAssert(json.RawMessage(`{}`))
	if spec.HasAnyRule() {
		t.Errorf("empty spec should HasAnyRule = false")
	}
}

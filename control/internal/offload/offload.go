// Package offload runs the expensive assertion tiers (JSON-path / regex,
// JSON Schema later) on sampled response bodies that workers shipped up.
//
// The hot path is the engine's per-request loop, which only does the cheap
// inline tiers (status / size / latency / content-type). Everything that
// requires parsing the body lives here.
package offload

import (
	"encoding/json"
	"fmt"
	"regexp"

	"github.com/tidwall/gjson"
)

// Verdict per response. Matches the bench schema's offload_eval.verdict CHECK
// constraint: pass | fail_schema | fail_value | fail_regex.
type Verdict string

const (
	VerdictPass       Verdict = "pass"
	VerdictFailSchema Verdict = "fail_schema"
	VerdictFailValue  Verdict = "fail_value"
	VerdictFailRegex  Verdict = "fail_regex"
)

// PathAssertion - one JSON-path value check.
//   - Path follows the gjson syntax (effectively a subset of JSONPath - dot
//     paths, indices, simple selectors). See https://github.com/tidwall/gjson.
//   - Equals: if set, the path's value must equal this (loose-typed compare:
//     number == number, string == string, bool == bool, null == JSON null).
//   - If Equals is nil, the assertion only checks the path EXISTS.
type PathAssertion struct {
	Path   string `json:"path"`
	Equals any    `json:"equals,omitempty"`
}

// Spec is the subset of AssertSpec the offload pool actually uses (the inline
// tiers are evaluated on the worker). Schema is reserved for the future
// JSON Schema tier.
type Spec struct {
	Paths    []PathAssertion `json:"paths,omitempty"`
	Patterns []string        `json:"patterns,omitempty"`
	Schema   json.RawMessage `json:"schema,omitempty"` // reserved
}

// Evaluator pre-compiles the regexes and applies the rules to one body at a
// time. Safe for concurrent use after construction.
type Evaluator struct {
	spec     Spec
	patterns []*regexp.Regexp
}

func NewEvaluator(spec Spec) (*Evaluator, error) {
	e := &Evaluator{spec: spec}
	for _, p := range spec.Patterns {
		re, err := regexp.Compile(p)
		if err != nil {
			return nil, fmt.Errorf("bad regex %q: %w", p, err)
		}
		e.patterns = append(e.patterns, re)
	}
	return e, nil
}

// Evaluate runs all configured tiers against `body` (raw response bytes,
// assumed valid UTF-8 if any path tier is configured). Returns the FIRST
// failure verdict, or VerdictPass when all tiers accept. `details` is a short
// human-readable explanation for the failure (empty on pass).
func (e *Evaluator) Evaluate(body []byte) (Verdict, string) {
	// Path tier (JSON-path / value).
	for _, pa := range e.spec.Paths {
		got := gjson.GetBytes(body, pa.Path)
		if !got.Exists() {
			return VerdictFailValue, fmt.Sprintf("path %q not present", pa.Path)
		}
		if pa.Equals == nil {
			continue
		}
		if !equalsLoose(got, pa.Equals) {
			return VerdictFailValue, fmt.Sprintf("path %q = %v, expected %v", pa.Path, got.Value(), pa.Equals)
		}
	}
	// Regex tier - every configured pattern must match somewhere in the body.
	for i, re := range e.patterns {
		if !re.Match(body) {
			return VerdictFailRegex, fmt.Sprintf("pattern %d %q did not match", i, e.spec.Patterns[i])
		}
	}
	return VerdictPass, ""
}

func equalsLoose(got gjson.Result, want any) bool {
	switch w := want.(type) {
	case string:
		return got.Type == gjson.String && got.String() == w
	case float64:
		return (got.Type == gjson.Number) && got.Float() == w
	case int:
		return (got.Type == gjson.Number) && got.Int() == int64(w)
	case int64:
		return (got.Type == gjson.Number) && got.Int() == w
	case bool:
		if w {
			return got.Type == gjson.True
		}
		return got.Type == gjson.False
	case nil:
		return got.Type == gjson.Null
	default:
		// Last-resort comparison via JSON encoding so callers can pass nested
		// structures via map[string]any from json.Unmarshal.
		wb, err := json.Marshal(w)
		if err != nil {
			return false
		}
		return string(wb) == got.Raw
	}
}

// HasAnyRule returns true if any tier is configured. Used to skip evaluation
// (and the cost of fetching the spec) when there's nothing to check.
func (s *Spec) HasAnyRule() bool {
	return len(s.Paths) > 0 || len(s.Patterns) > 0 || len(s.Schema) > 0
}

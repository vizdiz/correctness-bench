package api

import "testing"

func TestScrubHeaders_RedactsAuthByName(t *testing.T) {
	in := map[string]string{
		"Authorization": "Bearer sk-canary-abc123",
		"X-Api-Key":     "CANARY_DO_NOT_LEAK_xyz123",
		"Cookie":        "session=deadbeefdeadbeef",
		"Content-Type":  "application/json",
		"Accept":        "application/json",
	}
	out := ScrubHeaders(in)

	for _, name := range []string{"Authorization", "X-Api-Key", "Cookie"} {
		if out[name] != Redacted {
			t.Errorf("%s should be redacted, got %q", name, out[name])
		}
	}
	// Non-secret headers survive so the redacted spec is still re-runnable.
	if out["Content-Type"] != "application/json" {
		t.Errorf("Content-Type should survive, got %q", out["Content-Type"])
	}
	if out["Accept"] != "application/json" {
		t.Errorf("Accept should survive, got %q", out["Accept"])
	}
}

func TestScrubHeaders_RedactsTokenShapedValuesInCustomHeaders(t *testing.T) {
	in := map[string]string{
		"X-Custom-Token": "CANARY_DO_NOT_LEAK_xyz123", // long token-like -> redacted
		"X-Trace":        "abc",                        // short, benign -> kept
	}
	out := ScrubHeaders(in)
	if out["X-Custom-Token"] != Redacted {
		t.Errorf("token-shaped custom header should be redacted, got %q", out["X-Custom-Token"])
	}
	if out["X-Trace"] != "abc" {
		t.Errorf("short benign header should survive, got %q", out["X-Trace"])
	}
}

func TestScrubHeaders_NilSafe(t *testing.T) {
	out := ScrubHeaders(nil)
	if out == nil || len(out) != 0 {
		t.Errorf("nil input should yield empty non-nil map, got %v", out)
	}
}

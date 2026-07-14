package api

import (
	"bytes"
	"encoding/json"
	"log/slog"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/vizdiz/correctness-bench/control/internal/store"
)

// newHandler builds the real router over the given store, logging into buf.
func newHandler(st *store.Store) (http.Handler, *bytes.Buffer) {
	buf := &bytes.Buffer{}
	log := slog.New(slog.NewJSONHandler(buf, nil))
	return NewServer(st, log, "", "").Routes(), buf
}

func do(t *testing.T, h http.Handler, method, path, body string) *httptest.ResponseRecorder {
	t.Helper()
	req := httptest.NewRequest(method, path, strings.NewReader(body))
	req.Header.Set("Content-Type", "application/json")
	rec := httptest.NewRecorder()
	h.ServeHTTP(rec, req)
	return rec
}

const sampleRun = `{
  "name": "smoke",
  "target": { "url": "https://api.example.com/v1/foo", "method": "POST",
              "headers": { "Content-Type": "application/json" } },
  "target_rps": 100, "duration_s": 60, "load_model": "open",
  "connections": 50, "keepalive": true,
  "assert": { "expected_status": [200] },
  "rate_limit_policy": { "action": "backoff" }
}`

func TestCreateAndGetRun_RoundTrip(t *testing.T) {
	st := requireDB(t)
	h, _ := newHandler(st)

	rec := do(t, h, "POST", "/v1/runs", sampleRun)
	if rec.Code != http.StatusCreated {
		t.Fatalf("POST /v1/runs = %d, body=%s", rec.Code, rec.Body.String())
	}
	var created CreateRunResponse
	mustJSON(t, rec.Body.Bytes(), &created)
	if created.Status != "queued" || created.RunID == "" {
		t.Fatalf("unexpected create response: %+v", created)
	}

	rec = do(t, h, "GET", "/v1/runs/"+created.RunID, "")
	if rec.Code != http.StatusOK {
		t.Fatalf("GET run = %d, body=%s", rec.Code, rec.Body.String())
	}
	// The GET payload must NOT contain a headers field anywhere.
	if strings.Contains(rec.Body.String(), "headers") {
		t.Fatalf("GET response must not mention headers: %s", rec.Body.String())
	}
	var view RunView
	mustJSON(t, rec.Body.Bytes(), &view)
	if view.Status != "queued" || view.Target.URL == "" || view.Target.Method != "POST" {
		t.Fatalf("unexpected run view: %+v", view)
	}
}

func TestCreateRun_ValidationAndCost(t *testing.T) {
	st := requireDB(t)
	h, _ := newHandler(st)

	// Bad spec -> 400 INVALID_RUN_SPEC with field.
	rec := do(t, h, "POST", "/v1/runs", `{"target":{"url":"","method":"GET"},"target_rps":0}`)
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("expected 400, got %d", rec.Code)
	}

	// Over cost ceiling -> 400.
	over := `{"target":{"url":"https://x.io/a","method":"GET"},"target_rps":100,"duration_s":60,
	          "load_model":"open","connections":10,"keepalive":true,
	          "estimated_cost_usd":1.0,"cost_per_request_usd":0.01}` // projected 60 >> 1.1
	rec = do(t, h, "POST", "/v1/runs", over)
	if rec.Code != http.StatusBadRequest {
		t.Fatalf("expected 400 for cost ceiling, got %d body=%s", rec.Code, rec.Body.String())
	}
}

func TestGetRun_NotFound(t *testing.T) {
	st := requireDB(t)
	h, _ := newHandler(st)
	rec := do(t, h, "GET", "/v1/runs/00000000-0000-0000-0000-000000000000", "")
	if rec.Code != http.StatusNotFound {
		t.Fatalf("expected 404, got %d", rec.Code)
	}
	rec = do(t, h, "GET", "/v1/runs/not-a-uuid", "")
	if rec.Code != http.StatusNotFound {
		t.Fatalf("expected 404 for bad uuid, got %d", rec.Code)
	}
}

func TestAbortRun_TransitionsAndConflicts(t *testing.T) {
	st := requireDB(t)
	h, _ := newHandler(st)

	created := mustCreate(t, h)

	rec := do(t, h, "POST", "/v1/runs/"+created+"/abort", "")
	if rec.Code != http.StatusOK {
		t.Fatalf("abort = %d body=%s", rec.Code, rec.Body.String())
	}
	var ab AbortResponse
	mustJSON(t, rec.Body.Bytes(), &ab)
	if ab.Status != "aborted" || ab.AbortedAt == "" {
		t.Fatalf("unexpected abort response: %+v", ab)
	}

	// Second abort -> 409 CONFLICT (terminal state).
	rec = do(t, h, "POST", "/v1/runs/"+created+"/abort", "")
	if rec.Code != http.StatusConflict {
		t.Fatalf("expected 409 on re-abort, got %d", rec.Code)
	}

	// Unknown run -> 404.
	rec = do(t, h, "POST", "/v1/runs/00000000-0000-0000-0000-000000000000/abort", "")
	if rec.Code != http.StatusNotFound {
		t.Fatalf("expected 404 abort unknown, got %d", rec.Code)
	}
}

func TestListRuns_CursorPagination(t *testing.T) {
	st := requireDB(t)
	h, _ := newHandler(st)

	const n = 5
	for i := 0; i < n; i++ {
		mustCreate(t, h)
	}

	// Page size 2 -> expect cursor, then walk to exhaustion with no dupes.
	seen := map[string]bool{}
	cursor := ""
	pages := 0
	for {
		path := "/v1/runs?limit=2"
		if cursor != "" {
			path += "&cursor=" + cursor
		}
		rec := do(t, h, "GET", path, "")
		if rec.Code != http.StatusOK {
			t.Fatalf("list = %d body=%s", rec.Code, rec.Body.String())
		}
		var page ListRunsResponse
		mustJSON(t, rec.Body.Bytes(), &page)
		for _, r := range page.Runs {
			if seen[r.RunID] {
				t.Fatalf("duplicate run %s across pages", r.RunID)
			}
			seen[r.RunID] = true
		}
		pages++
		if page.NextCursor == nil {
			break
		}
		cursor = *page.NextCursor
		if pages > 10 {
			t.Fatal("pagination did not terminate")
		}
	}
	if len(seen) != n {
		t.Fatalf("expected %d distinct runs across pages, got %d", n, len(seen))
	}
}

func mustCreate(t *testing.T, h http.Handler) string {
	t.Helper()
	rec := do(t, h, "POST", "/v1/runs", sampleRun)
	if rec.Code != http.StatusCreated {
		t.Fatalf("create failed: %d %s", rec.Code, rec.Body.String())
	}
	var c CreateRunResponse
	mustJSON(t, rec.Body.Bytes(), &c)
	return c.RunID
}

func mustJSON(t *testing.T, b []byte, v any) {
	t.Helper()
	if err := json.Unmarshal(b, v); err != nil {
		t.Fatalf("json decode: %v (%s)", err, string(b))
	}
}

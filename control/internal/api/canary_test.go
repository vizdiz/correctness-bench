package api

import (
	"context"
	"net/http"
	"strings"
	"testing"
)

// Canary is a credential placed in target.headers. It must NEVER appear in the
// DB (any column, any row), in any API response, or in logs.
const Canary = "CANARY_DO_NOT_LEAK_xyz123"

// TestCredentialCanary is the hard credential-custody gate (.claude/agents/control.md).
// It injects the canary into multiple header positions, exercises the create +
// read path, then dumps every runs row to JSON text and greps it — plus the API
// responses and the captured logs — for the canary. Zero hits required.
func TestCredentialCanary(t *testing.T) {
	st := requireDB(t)
	h, logBuf := newHandler(st)
	ctx := context.Background()

	body := `{
      "name": "canary run",
      "target": {
        "url": "https://api.example.com/v1/foo",
        "method": "POST",
        "headers": {
          "Authorization": "Bearer ` + Canary + `",
          "X-Api-Key": "` + Canary + `",
          "X-Custom-Token": "` + Canary + `",
          "Content-Type": "application/json"
        }
      },
      "target_rps": 50, "duration_s": 30, "load_model": "open",
      "connections": 10, "keepalive": true,
      "assert": { "expected_status": [200] },
      "rate_limit_policy": { "action": "backoff" }
    }`

	rec := do(t, h, "POST", "/v1/runs", body)
	if rec.Code != http.StatusCreated {
		t.Fatalf("create = %d body=%s", rec.Code, rec.Body.String())
	}
	if strings.Contains(rec.Body.String(), Canary) {
		t.Fatalf("CANARY LEAKED in POST response: %s", rec.Body.String())
	}
	var created CreateRunResponse
	mustJSON(t, rec.Body.Bytes(), &created)

	// Read it back via the API.
	getRec := do(t, h, "GET", "/v1/runs/"+created.RunID, "")
	if strings.Contains(getRec.Body.String(), Canary) {
		t.Fatalf("CANARY LEAKED in GET response: %s", getRec.Body.String())
	}

	// Dump EVERY column of EVERY run row to JSON text and grep. row_to_json
	// covers all columns, so a leak in any column (including the redacted
	// headers / assert_spec / status_reason) is caught.
	rows, err := testPool.Query(ctx, `SELECT row_to_json(runs)::text FROM runs`)
	if err != nil {
		t.Fatalf("dump query: %v", err)
	}
	defer rows.Close()
	var dump strings.Builder
	for rows.Next() {
		var s string
		if err := rows.Scan(&s); err != nil {
			t.Fatalf("scan dump: %v", err)
		}
		dump.WriteString(s)
		dump.WriteString("\n")
	}
	if err := rows.Err(); err != nil {
		t.Fatalf("rows err: %v", err)
	}
	if strings.Contains(dump.String(), Canary) {
		t.Fatalf("CANARY LEAKED in DB dump: %s", dump.String())
	}

	// And the redacted headers must actually be redacted, not just absent —
	// prove the scrubbing happened rather than the column being empty.
	var headersText string
	if err := testPool.QueryRow(ctx,
		`SELECT target_headers_redacted::text FROM runs WHERE id = $1`, created.RunID,
	).Scan(&headersText); err != nil {
		t.Fatalf("read headers: %v", err)
	}
	if !strings.Contains(headersText, Redacted) {
		t.Fatalf("expected redacted placeholder in stored headers, got %q", headersText)
	}
	if strings.Contains(headersText, Canary) {
		t.Fatalf("CANARY LEAKED in target_headers_redacted: %q", headersText)
	}

	// Logs must never contain the canary either.
	if strings.Contains(logBuf.String(), Canary) {
		t.Fatalf("CANARY LEAKED in logs: %s", logBuf.String())
	}

	t.Logf("canary clean: 0 hits across POST/GET responses, full DB dump, and logs")
}

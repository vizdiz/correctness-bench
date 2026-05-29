// Package migrations embeds the goose migrations and exposes the raw schema.
package migrations

import (
	"embed"
	"strings"
)

//go:embed *.sql
var FS embed.FS

// RawSchema returns the SQL body of the initial migration with goose annotation
// lines (-- +goose ...) removed. Used by the test harness to apply the schema
// directly (with an optional Timescale shim) without going through goose.
func RawSchema() (string, error) {
	b, err := FS.ReadFile("00001_init.sql")
	if err != nil {
		return "", err
	}
	var out []string
	for _, line := range strings.Split(string(b), "\n") {
		trimmed := strings.TrimSpace(line)
		if strings.HasPrefix(trimmed, "-- +goose") {
			continue
		}
		// Drop the Down section entirely.
		if trimmed == "-- +goose Down" {
			break
		}
		out = append(out, line)
	}
	// Cut everything from the Down marker if present (defensive; the loop above
	// already breaks, but the marker line itself was a +goose line and skipped).
	joined := strings.Join(out, "\n")
	if idx := strings.Index(joined, "DROP TABLE IF EXISTS comparisons"); idx >= 0 {
		joined = joined[:idx]
	}
	return joined, nil
}

// TimescaleStatements are the exact statements in the frozen schema that require
// the timescaledb extension. The test harness skips these when timescaledb is
// not available (e.g. a plain Postgres used for CI of the credential path).
var TimescaleStatements = []string{
	"CREATE EXTENSION IF NOT EXISTS timescaledb;",
	"SELECT create_hypertable('run_ticks', 'ts', if_not_exists => TRUE);",
	"SELECT create_hypertable('worker_telemetry', 'ts', if_not_exists => TRUE);",
}

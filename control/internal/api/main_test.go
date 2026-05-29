package api

import (
	"context"
	"fmt"
	"os"
	"strings"
	"testing"

	"github.com/jackc/pgx/v5/pgxpool"

	"github.com/vizdiz/correctness-bench/control/internal/migrations"
	"github.com/vizdiz/correctness-bench/control/internal/store"
)

// testPool is non-nil only when a test database is configured via
// TEST_DATABASE_URL (or DATABASE_URL). DB-backed tests skip otherwise, so the
// suite stays green on a machine with no Postgres.
var testPool *pgxpool.Pool

func TestMain(m *testing.M) {
	ctx := context.Background()
	dsn := os.Getenv("TEST_DATABASE_URL")
	if dsn == "" {
		dsn = os.Getenv("DATABASE_URL")
	}
	if dsn != "" {
		pool, err := pgxpool.New(ctx, dsn)
		if err != nil {
			fmt.Println("DB tests skipped: pool:", err)
		} else if err := pool.Ping(ctx); err != nil {
			fmt.Println("DB tests skipped: ping:", err)
			pool.Close()
		} else if err := applyTestSchema(ctx, pool); err != nil {
			fmt.Println("DB schema setup FAILED:", err)
			pool.Close()
			os.Exit(1)
		} else {
			testPool = pool
		}
	} else {
		fmt.Println("DB tests skipped: set TEST_DATABASE_URL to enable")
	}

	code := m.Run()
	if testPool != nil {
		testPool.Close()
	}
	os.Exit(code)
}

// applyTestSchema applies the frozen schema to the test DB. If timescaledb is
// unavailable (e.g. a plain Postgres used for credential-path CI), the three
// Timescale-specific statements are shimmed out — the hypertables become plain
// tables, which is irrelevant to the credential/CRUD tests (they touch `runs`
// only). The shim asserts each statement still exists in the schema, so contract
// drift fails loudly instead of silently mis-applying.
func applyTestSchema(ctx context.Context, pool *pgxpool.Pool) error {
	schema, err := migrations.RawSchema()
	if err != nil {
		return err
	}
	if !timescaleAvailable(ctx, pool) {
		for _, stmt := range migrations.TimescaleStatements {
			if !strings.Contains(schema, stmt) {
				return fmt.Errorf("schema drift: timescale statement not found verbatim: %q", stmt)
			}
			schema = strings.Replace(schema, stmt, "-- [test-shim] skipped (no timescaledb): "+stmt, 1)
		}
		fmt.Println("note: timescaledb unavailable — applied schema with hypertables as plain tables (test shim)")
	}
	_, _ = pool.Exec(ctx, `DROP TABLE IF EXISTS comparisons, templates, offload_eval, worker_telemetry, run_ticks, runs CASCADE`)
	if _, err := pool.Exec(ctx, schema); err != nil {
		return fmt.Errorf("apply schema: %w", err)
	}
	return nil
}

func timescaleAvailable(ctx context.Context, pool *pgxpool.Pool) bool {
	var n int
	err := pool.QueryRow(ctx, `SELECT count(*) FROM pg_available_extensions WHERE name = 'timescaledb'`).Scan(&n)
	return err == nil && n > 0
}

// requireDB returns a store backed by the test pool, truncating runs first for
// isolation. Skips the test if no DB is configured.
func requireDB(t *testing.T) *store.Store {
	t.Helper()
	if testPool == nil {
		t.Skip("no test DB (set TEST_DATABASE_URL)")
	}
	if _, err := testPool.Exec(context.Background(), "TRUNCATE runs CASCADE"); err != nil {
		t.Fatalf("truncate: %v", err)
	}
	return store.NewFromPool(testPool)
}

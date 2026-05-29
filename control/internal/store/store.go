// Package store is the Postgres data layer (pgx). It owns all SQL; handlers
// never build queries. Target API keys never reach this package un-redacted.
package store

import (
	"context"
	"encoding/base64"
	"errors"
	"fmt"
	"strings"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgxpool"
)

var (
	ErrNotFound = errors.New("run not found")
	ErrConflict = errors.New("run is in a terminal state")
)

type Store struct {
	pool *pgxpool.Pool
}

func New(ctx context.Context, dsn string) (*Store, error) {
	pool, err := pgxpool.New(ctx, dsn)
	if err != nil {
		return nil, err
	}
	return &Store{pool: pool}, nil
}

func NewFromPool(pool *pgxpool.Pool) *Store { return &Store{pool: pool} }

func (s *Store) Close() { s.pool.Close() }

func (s *Store) Ping(ctx context.Context) error { return s.pool.Ping(ctx) }

// InsertRunParams carries already-validated, already-scrubbed values. The JSON
// fields are raw JSON text inserted into jsonb columns via an explicit ::jsonb cast.
type InsertRunParams struct {
	Name                *string
	TargetURL           string
	TargetMethod        string
	HeadersRedactedJSON string
	BodySize            *int
	TargetRPS           float64
	DurationS           int
	WarmupS             int
	LoadModel           string
	Connections         int
	Keepalive           bool
	AssertJSON          string
	RateLimitJSON       string
	EstimatedCostUSD    *float64
	CostPerRequestUSD   *float64
}

// RunRow is the read model for a single run.
type RunRow struct {
	ID           string
	Name         *string
	Status       string
	TargetURL    string
	TargetMethod string
	TargetRPS    float64
	DurationS    int
	EffectiveRPS *float64
	CreatedAt    time.Time
	StartedAt    *time.Time
	CompletedAt  *time.Time
}

// InsertRun inserts a queued run and returns its id + created_at.
func (s *Store) InsertRun(ctx context.Context, p InsertRunParams) (string, time.Time, error) {
	const q = `
INSERT INTO runs (
  name, target_url, target_method, target_headers_redacted, target_body_size,
  target_rps, duration_s, warmup_s, load_model, connections, keepalive,
  assert_spec, rate_limit_policy, estimated_cost_usd, cost_per_request_usd, status
) VALUES (
  $1, $2, $3, $4::jsonb, $5,
  $6, $7, $8, $9, $10, $11,
  $12::jsonb, $13::jsonb, $14, $15, 'queued'
)
RETURNING id, created_at`
	var id string
	var createdAt time.Time
	err := s.pool.QueryRow(ctx, q,
		p.Name, p.TargetURL, p.TargetMethod, p.HeadersRedactedJSON, p.BodySize,
		p.TargetRPS, p.DurationS, p.WarmupS, p.LoadModel, p.Connections, p.Keepalive,
		p.AssertJSON, p.RateLimitJSON, p.EstimatedCostUSD, p.CostPerRequestUSD,
	).Scan(&id, &createdAt)
	return id, createdAt, err
}

// GetRun reads a run by id. Returns ErrNotFound if it does not exist. Note: the
// query never selects target_headers_redacted — the API never needs it, so it
// cannot accidentally leak.
func (s *Store) GetRun(ctx context.Context, id string) (*RunRow, error) {
	const q = `
SELECT id, name, status, target_url, target_method, target_rps, duration_s,
       effective_rps, created_at, started_at, completed_at
FROM runs WHERE id = $1`
	var r RunRow
	err := s.pool.QueryRow(ctx, q, id).Scan(
		&r.ID, &r.Name, &r.Status, &r.TargetURL, &r.TargetMethod, &r.TargetRPS, &r.DurationS,
		&r.EffectiveRPS, &r.CreatedAt, &r.StartedAt, &r.CompletedAt,
	)
	if errors.Is(err, pgx.ErrNoRows) {
		return nil, ErrNotFound
	}
	if err != nil {
		return nil, err
	}
	return &r, nil
}

// ListRuns returns up to limit runs ordered by (created_at, id) descending,
// using keyset (cursor) pagination. status, if non-empty, filters. nextCursor is
// empty when there are no more pages.
func (s *Store) ListRuns(ctx context.Context, status string, limit int, cursor string) ([]RunRow, string, error) {
	if limit <= 0 || limit > 100 {
		limit = 20
	}

	var (
		conds []string
		args  []any
	)
	add := func(a any) string { args = append(args, a); return fmt.Sprintf("$%d", len(args)) }

	if status != "" {
		conds = append(conds, "status = "+add(status))
	}
	if cursor != "" {
		ts, cid, err := decodeCursor(cursor)
		if err != nil {
			return nil, "", fmt.Errorf("bad cursor: %w", err)
		}
		conds = append(conds, fmt.Sprintf("(created_at, id) < (%s, %s)", add(ts), add(cid)))
	}

	where := ""
	if len(conds) > 0 {
		where = "WHERE " + strings.Join(conds, " AND ")
	}
	q := fmt.Sprintf(`
SELECT id, name, status, target_url, target_method, target_rps, duration_s,
       effective_rps, created_at, started_at, completed_at
FROM runs %s
ORDER BY created_at DESC, id DESC
LIMIT %s`, where, add(limit+1)) // fetch one extra to detect a next page

	rows, err := s.pool.Query(ctx, q, args...)
	if err != nil {
		return nil, "", err
	}
	defer rows.Close()

	var out []RunRow
	for rows.Next() {
		var r RunRow
		if err := rows.Scan(
			&r.ID, &r.Name, &r.Status, &r.TargetURL, &r.TargetMethod, &r.TargetRPS, &r.DurationS,
			&r.EffectiveRPS, &r.CreatedAt, &r.StartedAt, &r.CompletedAt,
		); err != nil {
			return nil, "", err
		}
		out = append(out, r)
	}
	if err := rows.Err(); err != nil {
		return nil, "", err
	}

	nextCursor := ""
	if len(out) > limit {
		last := out[limit-1]
		nextCursor = encodeCursor(last.CreatedAt, last.ID)
		out = out[:limit]
	}
	return out, nextCursor, nil
}

// AbortRun transitions a run to 'aborted' if it is not already terminal.
// Returns ErrNotFound or ErrConflict as appropriate. Worker propagation is
// engine work and intentionally not done here.
func (s *Store) AbortRun(ctx context.Context, id string) (time.Time, error) {
	tx, err := s.pool.Begin(ctx)
	if err != nil {
		return time.Time{}, err
	}
	defer tx.Rollback(ctx)

	var status string
	err = tx.QueryRow(ctx, `SELECT status FROM runs WHERE id = $1 FOR UPDATE`, id).Scan(&status)
	if errors.Is(err, pgx.ErrNoRows) {
		return time.Time{}, ErrNotFound
	}
	if err != nil {
		return time.Time{}, err
	}
	switch status {
	case "completed", "failed", "aborted":
		return time.Time{}, ErrConflict
	}

	var abortedAt time.Time
	err = tx.QueryRow(ctx,
		`UPDATE runs SET status='aborted', status_reason='aborted by request', completed_at=now()
		 WHERE id=$1 RETURNING completed_at`, id).Scan(&abortedAt)
	if err != nil {
		return time.Time{}, err
	}
	if err := tx.Commit(ctx); err != nil {
		return time.Time{}, err
	}
	return abortedAt, nil
}

// ---- cursor encoding: opaque base64 of "RFC3339Nano|uuid" ----

func encodeCursor(ts time.Time, id string) string {
	raw := ts.UTC().Format(time.RFC3339Nano) + "|" + id
	return base64.RawURLEncoding.EncodeToString([]byte(raw))
}

func decodeCursor(c string) (time.Time, string, error) {
	b, err := base64.RawURLEncoding.DecodeString(c)
	if err != nil {
		return time.Time{}, "", err
	}
	parts := strings.SplitN(string(b), "|", 2)
	if len(parts) != 2 {
		return time.Time{}, "", errors.New("malformed cursor")
	}
	ts, err := time.Parse(time.RFC3339Nano, parts[0])
	if err != nil {
		return time.Time{}, "", err
	}
	return ts, parts[1], nil
}

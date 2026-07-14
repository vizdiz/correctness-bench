package store

import (
	"context"
	"time"
)

// InsertTickParams is one per-second tick mapped onto the run_ticks columns.
// The caller (ingest path) maps the transport tick shape into this; store never
// imports the transport package.
type InsertTickParams struct {
	RunID           string
	ElapsedS        int64
	AchievedRPS     float64
	P50US           int64
	P99US           int64
	BucketsJSON     string // JSON array text; "" -> NULL
	Total           int64
	Pass            int64
	FailStatus      int64
	FailLatency     int64
	FailSize        int64
	FailContentType int64
	RateLimited     int64
}

// InsertTick persists one tick to the run_ticks hypertable. Called async from
// the ingest path so a slow write never stalls tick fan-out (schema rule #4).
func (s *Store) InsertTick(ctx context.Context, p InsertTickParams) error {
	const q = `
INSERT INTO run_ticks (
  run_id, ts, elapsed_s, offered_rps, achieved_rps, p50_us, p99_us, buckets,
  total, pass, fail_status, fail_latency, fail_size, fail_content_type, rate_limited
) VALUES (
  $1, $2, $3, $4, $4, $5, $6, NULLIF($7, '')::jsonb,
  $8, $9, $10, $11, $12, $13, $14
)`
	_, err := s.pool.Exec(ctx, q,
		p.RunID, time.Now().UTC(), p.ElapsedS, p.AchievedRPS, p.P50US, p.P99US, p.BucketsJSON,
		p.Total, p.Pass, p.FailStatus, p.FailLatency, p.FailSize, p.FailContentType, p.RateLimited,
	)
	return err
}

// TickRow is the read model for one persisted tick.
type TickRow struct {
	ElapsedS        int64
	AchievedRPS     *float64
	P50US           *int64
	P99US           *int64
	BucketsJSON     []byte
	Total           *int64
	Pass            *int64
	FailStatus      *int64
	FailLatency     *int64
	FailSize        *int64
	FailContentType *int64
	RateLimited     *int64
}

// GetTicks returns a run's persisted per-second ticks ordered by elapsed_s.
// Empty (not an error) for a run that never streamed any.
func (s *Store) GetTicks(ctx context.Context, runID string) ([]TickRow, error) {
	const q = `
SELECT elapsed_s, achieved_rps, p50_us, p99_us, buckets,
       total, pass, fail_status, fail_latency, fail_size, fail_content_type, rate_limited
FROM run_ticks WHERE run_id = $1 ORDER BY elapsed_s ASC`
	rows, err := s.pool.Query(ctx, q, runID)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []TickRow
	for rows.Next() {
		var t TickRow
		if err := rows.Scan(
			&t.ElapsedS, &t.AchievedRPS, &t.P50US, &t.P99US, &t.BucketsJSON,
			&t.Total, &t.Pass, &t.FailStatus, &t.FailLatency, &t.FailSize, &t.FailContentType, &t.RateLimited,
		); err != nil {
			return nil, err
		}
		out = append(out, t)
	}
	return out, rows.Err()
}

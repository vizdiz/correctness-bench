package store

import (
	"context"
	"crypto/sha256"
	"encoding/json"
	"errors"
	"time"

	"github.com/jackc/pgx/v5"
)

// GetAssertSpec returns the raw `assert_spec` JSONB of a run. ErrNotFound if
// the run doesn't exist.
func (s *Store) GetAssertSpec(ctx context.Context, runID string) (json.RawMessage, error) {
	var raw []byte
	err := s.pool.QueryRow(ctx,
		`SELECT assert_spec::text FROM runs WHERE id = $1`, runID,
	).Scan(&raw)
	if errors.Is(err, pgx.ErrNoRows) {
		return nil, ErrNotFound
	}
	if err != nil {
		return nil, err
	}
	return json.RawMessage(raw), nil
}

// InsertOffloadVerdictParams carries one verdict to persist.
type InsertOffloadVerdictParams struct {
	RunID          string
	RpsAtSend      float64
	Status         int32
	CorrectedLatUS int64
	Verdict        string // pass | fail_schema | fail_value | fail_regex
	// DetailsJSON: JSON text for the details JSONB column. "" becomes SQL NULL.
	DetailsJSON string
	BodySize    int32
	BodySHA256  []byte
	Body        []byte // optional; pass nil to omit
}

// InsertOffloadVerdict appends one row to offload_eval. body_sha256 is always
// stored (for dedup / proof); body itself is stored only when non-nil.
func (s *Store) InsertOffloadVerdict(ctx context.Context, p InsertOffloadVerdictParams) error {
	const q = `
INSERT INTO offload_eval (
    run_id, ts, rps_at_send, status, corrected_lat_us,
    verdict, details, body_size, body_sha256, body
) VALUES (
    $1, $2, $3, $4, $5,
    $6, NULLIF($7, '')::jsonb, $8, $9, $10
)`
	_, err := s.pool.Exec(ctx, q,
		p.RunID,
		time.Now().UTC(),
		p.RpsAtSend,
		p.Status,
		p.CorrectedLatUS,
		p.Verdict,
		p.DetailsJSON,
		p.BodySize,
		p.BodySHA256,
		p.Body,
	)
	return err
}

// OffloadVerdictCounts is the per-verdict roll-up shown alongside the inline
// fail-by-tier counts.
type OffloadVerdictCounts struct {
	Pass       int64 `json:"pass"`
	FailSchema int64 `json:"fail_schema"`
	FailValue  int64 `json:"fail_value"`
	FailRegex  int64 `json:"fail_regex"`
}

func (s *Store) OffloadCountsForRun(ctx context.Context, runID string) (OffloadVerdictCounts, error) {
	const q = `
SELECT verdict, COUNT(*) FROM offload_eval
WHERE run_id = $1
GROUP BY verdict`
	rows, err := s.pool.Query(ctx, q, runID)
	if err != nil {
		return OffloadVerdictCounts{}, err
	}
	defer rows.Close()
	var c OffloadVerdictCounts
	for rows.Next() {
		var v string
		var n int64
		if err := rows.Scan(&v, &n); err != nil {
			return OffloadVerdictCounts{}, err
		}
		switch v {
		case "pass":
			c.Pass = n
		case "fail_schema":
			c.FailSchema = n
		case "fail_value":
			c.FailValue = n
		case "fail_regex":
			c.FailRegex = n
		}
	}
	return c, rows.Err()
}

// SHA256Body is a small helper so callers don't reach into crypto/sha256 in
// hot paths repeatedly.
func SHA256Body(body []byte) []byte {
	sum := sha256.Sum256(body)
	return sum[:]
}

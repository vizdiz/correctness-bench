package store

import "context"

// MarkRunning transitions a non-terminal run to running and stamps started_at.
// No-op if the run already advanced past queued (idempotent, race-safe).
func (s *Store) MarkRunning(ctx context.Context, id string) error {
	_, err := s.pool.Exec(ctx, `
UPDATE runs SET status = 'running', started_at = COALESCE(started_at, now())
WHERE id = $1 AND status IN ('draft', 'validated', 'queued')`, id)
	return err
}

// MarkFailed transitions a non-terminal run to failed with a reason. No-op if
// the run already reached a terminal state (e.g. finalize won the race).
func (s *Store) MarkFailed(ctx context.Context, id, reason string) error {
	_, err := s.pool.Exec(ctx, `
UPDATE runs SET status = 'failed', status_reason = $2, completed_at = now()
WHERE id = $1 AND status NOT IN ('completed', 'aborted', 'failed')`, id, reason)
	return err
}

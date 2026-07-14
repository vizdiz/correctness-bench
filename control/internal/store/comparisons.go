package store

import "context"

// InsertComparison records a saved comparison set (the runs that were fired
// together under one epoch). Returns the comparison id.
func (s *Store) InsertComparison(ctx context.Context, name string, runIDs []string) (string, error) {
	var id string
	err := s.pool.QueryRow(ctx,
		`INSERT INTO comparisons (name, run_ids) VALUES ($1, $2) RETURNING id`,
		name, runIDs,
	).Scan(&id)
	return id, err
}

// RunsWereComparedTogether reports whether some comparison set contains BOTH
// runs - i.e. they were dispatched concurrently under one schedule, so the
// comparison is fair (no sequential time confound).
func (s *Store) RunsWereComparedTogether(ctx context.Context, a, b string) (bool, error) {
	var ok bool
	err := s.pool.QueryRow(ctx,
		`SELECT EXISTS(SELECT 1 FROM comparisons WHERE $1 = ANY(run_ids) AND $2 = ANY(run_ids))`,
		a, b,
	).Scan(&ok)
	return ok, err
}

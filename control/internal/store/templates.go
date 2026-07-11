package store

import (
	"context"
	"encoding/json"
	"errors"
	"time"

	"github.com/jackc/pgx/v5"
)

// TemplateRow is the read model for one row in the `templates` table. The
// SpecRedacted field is the run-spec JSONB with auth-shaped header values
// already replaced by "***" - it never carries secrets.
type TemplateRow struct {
	ID            string
	Name          string
	SpecRedacted  json.RawMessage
	CreatedAt     time.Time
	LastUsedAt    *time.Time
}

// InsertTemplate stores a redacted run-spec under `name` and returns the new id.
func (s *Store) InsertTemplate(ctx context.Context, name string, specRedacted json.RawMessage) (string, time.Time, error) {
	const q = `
INSERT INTO templates (name, spec_redacted)
VALUES ($1, $2::jsonb)
RETURNING id, created_at`
	var id string
	var createdAt time.Time
	err := s.pool.QueryRow(ctx, q, name, string(specRedacted)).Scan(&id, &createdAt)
	return id, createdAt, err
}

// GetTemplate returns one template by id, or ErrNotFound.
func (s *Store) GetTemplate(ctx context.Context, id string) (*TemplateRow, error) {
	const q = `
SELECT id, name, spec_redacted::text, created_at, last_used_at
FROM templates WHERE id = $1`
	var r TemplateRow
	var specText string
	err := s.pool.QueryRow(ctx, q, id).Scan(
		&r.ID, &r.Name, &specText, &r.CreatedAt, &r.LastUsedAt,
	)
	if errors.Is(err, pgx.ErrNoRows) {
		return nil, ErrNotFound
	}
	if err != nil {
		return nil, err
	}
	r.SpecRedacted = json.RawMessage(specText)
	return &r, nil
}

// ListTemplates returns templates ordered by created_at desc, capped at `limit`.
func (s *Store) ListTemplates(ctx context.Context, limit int) ([]TemplateRow, error) {
	if limit <= 0 || limit > 200 {
		limit = 50
	}
	const q = `
SELECT id, name, spec_redacted::text, created_at, last_used_at
FROM templates
ORDER BY created_at DESC
LIMIT $1`
	rows, err := s.pool.Query(ctx, q, limit)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var out []TemplateRow
	for rows.Next() {
		var r TemplateRow
		var specText string
		if err := rows.Scan(&r.ID, &r.Name, &specText, &r.CreatedAt, &r.LastUsedAt); err != nil {
			return nil, err
		}
		r.SpecRedacted = json.RawMessage(specText)
		out = append(out, r)
	}
	return out, rows.Err()
}

// MarkTemplateUsed bumps last_used_at on the row. Best-effort - missing rows
// return ErrNotFound but most callers can swallow this.
func (s *Store) MarkTemplateUsed(ctx context.Context, id string) error {
	tag, err := s.pool.Exec(ctx,
		`UPDATE templates SET last_used_at = now() WHERE id = $1`, id)
	if err != nil {
		return err
	}
	if tag.RowsAffected() == 0 {
		return ErrNotFound
	}
	return nil
}

package store

import (
	"database/sql"
	"fmt"

	_ "github.com/jackc/pgx/v5/stdlib" // pgx database/sql driver for goose
	"github.com/pressly/goose/v3"

	"github.com/vizdiz/correctness-bench/control/internal/migrations"
)

// Migrate applies all pending migrations using goose over a database/sql
// connection (pgx stdlib driver). This is the production path and runs the
// frozen schema verbatim, including the timescaledb extension + hypertables.
func Migrate(dsn string) error {
	db, err := sql.Open("pgx", dsn)
	if err != nil {
		return fmt.Errorf("open for migrate: %w", err)
	}
	defer db.Close()

	goose.SetBaseFS(migrations.FS)
	if err := goose.SetDialect("postgres"); err != nil {
		return err
	}
	if err := goose.Up(db, "."); err != nil {
		return fmt.Errorf("goose up: %w", err)
	}
	return nil
}

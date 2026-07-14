// Package config loads control-plane configuration from the environment.
package config

import "os"

type Config struct {
	// DatabaseURL is a libpq-style DSN consumed by pgx.
	DatabaseURL string
	// Addr is the listen address for the HTTP server, e.g. ":8000".
	Addr string
	// AutoMigrate runs goose migrations on startup. Default true (convenient for
	// docker-compose); set AUTO_MIGRATE=false when migrations run as a separate
	// deploy step or the schema is already applied.
	AutoMigrate bool
	// CoordinatorURL is the coordinator's admin HTTP base; control POSTs runs
	// there to fire the fleet.
	CoordinatorURL string
	// SelfURL is control's own base URL as reachable FROM the coordinator, used
	// to build the tick/finalize callback URLs the coordinator pushes to.
	SelfURL string
}

func Load() Config {
	return Config{
		DatabaseURL: getenv("DATABASE_URL", "postgres://bench:bench@localhost:5432/bench?sslmode=disable"),
		Addr:        getenv("CONTROL_ADDR", ":8000"),
		AutoMigrate: getenv("AUTO_MIGRATE", "true") != "false",
		CoordinatorURL: getenv("COORDINATOR_ADMIN_URL", "http://localhost:9091"),
		SelfURL:        getenv("CONTROL_SELF_URL", "http://localhost:8000"),
	}
}

func getenv(k, def string) string {
	if v := os.Getenv(k); v != "" {
		return v
	}
	return def
}

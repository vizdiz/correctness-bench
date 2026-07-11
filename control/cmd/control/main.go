// Command control is the benchmarker control plane: lifecycle, REST API,
// credential custody, persistence. It never generates load.
package main

import (
	"context"
	"errors"
	"log/slog"
	"net/http"
	"os"
	"os/signal"
	"syscall"
	"time"

	"github.com/vizdiz/correctness-bench/control/internal/api"
	"github.com/vizdiz/correctness-bench/control/internal/config"
	"github.com/vizdiz/correctness-bench/control/internal/store"
)

func main() {
	log := slog.New(slog.NewJSONHandler(os.Stdout, nil))
	cfg := config.Load()

	// Apply migrations (frozen schema) before serving, unless disabled.
	if cfg.AutoMigrate {
		if err := store.Migrate(cfg.DatabaseURL); err != nil {
			log.Error("migration failed", "err", err.Error())
			os.Exit(1)
		}
		log.Info("migrations applied")
	} else {
		log.Info("auto-migrate disabled; assuming schema already applied")
	}

	ctx := context.Background()

	st, err := store.New(ctx, cfg.DatabaseURL)
	if err != nil {
		log.Error("db connect failed", "err", err.Error())
		os.Exit(1)
	}
	defer st.Close()

	srv := &http.Server{
		Addr:              cfg.Addr,
		Handler:           api.NewServer(st, log).Routes(),
		ReadHeaderTimeout: 5 * time.Second,
	}

	go func() {
		log.Info("control plane listening", "addr", cfg.Addr)
		if err := srv.ListenAndServe(); err != nil && !errors.Is(err, http.ErrServerClosed) {
			log.Error("server error", "err", err.Error())
			os.Exit(1)
		}
	}()

	stop := make(chan os.Signal, 1)
	signal.Notify(stop, os.Interrupt, syscall.SIGTERM)
	<-stop

	log.Info("shutting down")
	shutdownCtx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	_ = srv.Shutdown(shutdownCtx)
}

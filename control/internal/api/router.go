package api

import (
	"net/http"

	"github.com/go-chi/chi/v5"
	"github.com/go-chi/chi/v5/middleware"
)

// Routes wires the v1 surface. Endpoints implemented:
//   POST /v1/runs, GET /v1/runs, GET /v1/runs/{id}, POST /v1/runs/{id}/abort,
//   GET /v1/runs/{id}/stream (SSE — api.md),
//   POST /v1/_internal/runs/{id}/tick (v0 ingest from engine workers; not part
//   of the public api.md surface — will be replaced by bench.proto coordinator).
// Deliberately NOT wired (depend on engine data we don't have yet):
//   /histogram, /compare, /regression-check, templates.
func (s *Server) Routes() http.Handler {
	r := chi.NewRouter()
	r.Use(middleware.RequestID)
	r.Use(middleware.Recoverer)

	r.Get("/healthz", s.Health)

	r.Route("/v1", func(r chi.Router) {
		r.Route("/runs", func(r chi.Router) {
			r.Post("/", s.CreateRun)
			r.Get("/", s.ListRuns)
			r.Get("/{id}", s.GetRun)
			r.Post("/{id}/abort", s.AbortRun)
			r.Get("/{id}/stream", s.StreamRun)
		})
		r.Route("/_internal/runs/{id}", func(r chi.Router) {
			r.Post("/tick", s.IngestTick)
		})
	})

	return r
}

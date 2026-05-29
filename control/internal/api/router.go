package api

import (
	"net/http"

	"github.com/go-chi/chi/v5"
	"github.com/go-chi/chi/v5/middleware"
)

// Routes wires the v1 surface. Endpoints implemented this phase:
//   POST /v1/runs, GET /v1/runs, GET /v1/runs/{id}, POST /v1/runs/{id}/abort.
// Deliberately NOT wired (depend on the engine / out of scope for now):
//   GET /v1/runs/{id}/stream (SSE), /histogram, /compare, /regression-check, templates.
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
		})
	})

	return r
}

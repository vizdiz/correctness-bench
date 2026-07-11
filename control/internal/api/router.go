package api

import (
	"net/http"

	"github.com/go-chi/chi/v5"
	"github.com/go-chi/chi/v5/middleware"
)

// Routes wires the v1 surface. Endpoints implemented:
//   POST /v1/runs, GET /v1/runs, GET /v1/runs/{id}, POST /v1/runs/{id}/abort,
//   GET /v1/runs/{id}/stream (SSE — api.md),
//   GET /v1/runs/{id}/histogram, GET /v1/runs/{id}/compare/{id2},
//   POST /v1/runs/{id}/regression-check,
//   POST /v1/templates, GET /v1/templates, POST /v1/templates/{id}/run,
//   POST /v1/_internal/runs/{id}/tick + /finalize (v0 ingest from engine
//   workers; not part of the public api.md surface — will be replaced by
//   the bench.proto coordinator path).
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
			r.Get("/{id}/histogram", s.GetHistogram)
			r.Get("/{id}/compare/{id2}", s.Compare)
			r.Post("/{id}/regression-check", s.RegressionCheck)
		})
		r.Route("/_internal/runs/{id}", func(r chi.Router) {
			r.Post("/tick", s.IngestTick)
			r.Post("/finalize", s.FinalizeRun)
		})
		r.Route("/templates", func(r chi.Router) {
			r.Post("/", s.CreateTemplate)
			r.Get("/", s.ListTemplates)
			r.Post("/{id}/run", s.RunFromTemplate)
		})
	})

	return r
}

package api

import (
	"encoding/base64"
	"errors"
	"math"
	"net/http"

	"github.com/HdrHistogram/hdrhistogram-go"
	"github.com/go-chi/chi/v5"
	"github.com/google/uuid"

	"github.com/vizdiz/correctness-bench/control/internal/store"
)

// HistogramJSONResponse is the binned form of the final HDR shipped by the
// engine. Bins are log-spaced across the value range so a sparse 60-second
// HDR fits in a small JSON without losing the tail.
type HistogramJSONResponse struct {
	Bins   []HistogramBin `json:"bins"`
	P50US  int64          `json:"p50_us"`
	P95US  int64          `json:"p95_us"`
	P99US  int64          `json:"p99_us"`
	P999US int64          `json:"p999_us"`
	Total  int64          `json:"total"`
	MinUS  int64          `json:"min_us"`
	MaxUS  int64          `json:"max_us"`
}

type HistogramBin struct {
	LoUS  int64 `json:"lo_us"`
	HiUS  int64 `json:"hi_us"`
	Count int64 `json:"count"`
}

const histogramBinCount = 48

// histogramJSON renders the persisted V2-deflate bytes into a binned JSON the
// web can plot without a client-side HDR decoder. Shared by both the corrected
// and uncorrected variants — `bytes` is whichever the caller chose.
func histogramJSON(raw []byte) (HistogramJSONResponse, error) {
	if len(raw) == 0 {
		return HistogramJSONResponse{}, errors.New("empty histogram")
	}
	// hdrhistogram-go's Decode wants a BASE64 string in []byte form. Our
	// store column already holds raw V2-deflate bytes, so encode first.
	encoded := []byte(base64.StdEncoding.EncodeToString(raw))
	h, err := hdrhistogram.Decode(encoded)
	if err != nil {
		return HistogramJSONResponse{}, err
	}
	total := h.TotalCount()
	resp := HistogramJSONResponse{
		P50US:  h.ValueAtQuantile(50),
		P95US:  h.ValueAtQuantile(95),
		P99US:  h.ValueAtQuantile(99),
		P999US: h.ValueAtQuantile(99.9),
		Total:  total,
	}
	if total == 0 {
		resp.Bins = []HistogramBin{}
		return resp, nil
	}

	// Range we care about: actual min..max from the HDR. For log spacing we
	// take ln(lo)..ln(hi) and step linearly across that.
	bars := h.Distribution()
	minVal := int64(math.MaxInt64)
	maxVal := int64(0)
	for _, b := range bars {
		if b.Count == 0 {
			continue
		}
		if b.From < minVal {
			minVal = b.From
		}
		if b.To > maxVal {
			maxVal = b.To
		}
	}
	if minVal < 1 {
		minVal = 1
	}
	if maxVal <= minVal {
		maxVal = minVal + 1
	}
	resp.MinUS = minVal
	resp.MaxUS = maxVal

	logLo := math.Log(float64(minVal))
	logHi := math.Log(float64(maxVal))
	step := (logHi - logLo) / float64(histogramBinCount)
	bins := make([]HistogramBin, histogramBinCount)
	for i := 0; i < histogramBinCount; i++ {
		bins[i].LoUS = int64(math.Exp(logLo + step*float64(i)))
		bins[i].HiUS = int64(math.Exp(logLo + step*float64(i+1)))
		// Last bin always extends to the observed max so totals reconcile.
		if i == histogramBinCount-1 {
			bins[i].HiUS = maxVal
		}
	}
	// Fold each HDR bar's count into the bin whose midpoint contains it.
	for _, b := range bars {
		if b.Count == 0 {
			continue
		}
		mid := (b.From + b.To) / 2
		idx := int(((math.Log(float64(mid))) - logLo) / step)
		if idx < 0 {
			idx = 0
		}
		if idx >= histogramBinCount {
			idx = histogramBinCount - 1
		}
		bins[idx].Count += b.Count
	}
	resp.Bins = bins
	return resp, nil
}

// GetHistogramJSON: GET /v1/runs/:id/histogram?format=json[&which=corrected|uncorrected].
// Default `which` is corrected. Returns 409 when the histogram hasn't been
// finalized for this run, 404 when the run is missing.
func (s *Server) GetHistogramJSON(w http.ResponseWriter, r *http.Request) {
	id := chi.URLParam(r, "id")
	if _, err := uuid.Parse(id); err != nil {
		writeError(w, http.StatusNotFound, CodeNotFound, "run not found", "")
		return
	}
	which := r.URL.Query().Get("which")
	if which == "" {
		which = "corrected"
	}
	var bytes []byte
	var err error
	switch which {
	case "corrected":
		bytes, err = s.Store.GetFinalHistogram(r.Context(), id)
	case "uncorrected":
		bytes, err = s.Store.GetFinalUncorrectedHistogram(r.Context(), id)
	default:
		writeError(w, http.StatusBadRequest, CodeInvalidRunSpec,
			"which must be corrected or uncorrected", "which")
		return
	}
	if errors.Is(err, store.ErrNotFound) {
		writeError(w, http.StatusNotFound, CodeNotFound, "run not found", "")
		return
	}
	if err != nil {
		s.internal(w, "get histogram", err)
		return
	}
	if len(bytes) == 0 {
		writeError(w, http.StatusConflict, CodeConflict,
			"histogram not yet finalized for this run", "")
		return
	}
	parsed, err := histogramJSON(bytes)
	if err != nil {
		s.internal(w, "decode histogram", err)
		return
	}
	writeJSON(w, http.StatusOK, parsed)
}

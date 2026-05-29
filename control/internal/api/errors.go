package api

import (
	"encoding/json"
	"net/http"

	"github.com/google/uuid"
)

// Error codes per contracts/api.md.
const (
	CodeInvalidRunSpec = "INVALID_RUN_SPEC" // 400
	CodeNotFound       = "NOT_FOUND"        // 404
	CodeConflict       = "CONFLICT"         // 409
	CodeRateLimited    = "RATE_LIMITED"     // 429
	CodeInternal       = "INTERNAL"         // 500
	CodeNoCapacity     = "NO_CAPACITY"      // 503
)

type errorBody struct {
	Code      string `json:"code"`
	Message   string `json:"message"`
	Field     string `json:"field,omitempty"`
	RequestID string `json:"request_id"`
}

type apiError struct {
	Error errorBody `json:"error"`
}

// writeError emits the api.md error envelope with the matching HTTP status.
func writeError(w http.ResponseWriter, status int, code, message, field string) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(apiError{Error: errorBody{
		Code:      code,
		Message:   message,
		Field:     field,
		RequestID: uuid.NewString(),
	}})
}

func writeJSON(w http.ResponseWriter, status int, v any) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(v)
}

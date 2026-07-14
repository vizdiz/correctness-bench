package api

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"strings"
	"time"
)

// coordRunRequest mirrors the coordinator's admin POST /admin/runs body
// (engine coordinator/admin.rs RunRequest). Field names must match exactly.
type coordRunRequest struct {
	RunID              string            `json:"run_id"`
	TargetURL          string            `json:"target_url"`
	TargetMethod       string            `json:"target_method"`
	TargetRPS          float64           `json:"target_rps"`
	DurationS          int               `json:"duration_s"`
	Connections        int               `json:"connections"`
	Keepalive          bool              `json:"keepalive"`
	TimeoutMS          int               `json:"timeout_ms"`
	ExpectedStatus     []int32           `json:"expected_status"`
	MaxLatencyUS       *int64            `json:"max_latency_us,omitempty"`
	MinBodyBytes       *int32            `json:"min_body_bytes,omitempty"`
	MaxBodyBytes       *int32            `json:"max_body_bytes,omitempty"`
	ContentType        *string           `json:"content_type,omitempty"`
	TargetHeaders      map[string]string `json:"target_headers,omitempty"`
	EpochUnixUS        int64             `json:"epoch_unix_us,omitempty"`
	ControlTickURL     string            `json:"control_tick_url,omitempty"`
	ControlFinalizeURL string            `json:"control_finalize_url,omitempty"`
}

// assertFields is the slice of the assert spec the coordinator's inline tier
// needs. Schema/paths/patterns stay in control's offload pool, not the wire.
type assertFields struct {
	ExpectedStatus []int32 `json:"expected_status"`
	MaxLatencyUS   *int64  `json:"max_latency_us"`
	MinBodyBytes   *int32  `json:"min_body_bytes"`
	MaxBodyBytes   *int32  `json:"max_body_bytes"`
	ContentType    *string `json:"content_type"`
}

// dispatchToCoordinator POSTs a created run to the coordinator so the fleet
// fires; the coordinator then streams ticks + finalize back to control. This
// blocks for the whole run (the coordinator holds the connection), so it is
// ALWAYS called in its own goroutine. Target headers ride in memory only and
// are never persisted or logged.
// dispatchToCoordinator fires a run at the coordinator. epochUnixUs of 0 lets
// the coordinator start the schedule now; a shared non-zero epoch makes two
// runs share one window (fair concurrent comparison).
func (s *Server) dispatchToCoordinator(runID string, req *CreateRunRequest, epochUnixUs int64) {
	if err := s.Store.MarkRunning(context.Background(), runID); err != nil {
		s.Log.Warn("mark running", "run_id", runID, "err", err.Error())
	}

	var a assertFields
	if len(req.Assert) > 0 {
		_ = json.Unmarshal(req.Assert, &a)
	}
	keepalive := true
	if req.Keepalive != nil {
		keepalive = *req.Keepalive
	}
	timeout := req.Target.TimeoutMS
	if timeout == 0 {
		timeout = 30000
	}
	expected := a.ExpectedStatus
	if expected == nil {
		expected = []int32{}
	}
	self := strings.TrimRight(s.SelfURL, "/")

	payload, err := json.Marshal(coordRunRequest{
		RunID:              runID,
		TargetURL:          req.Target.URL,
		TargetMethod:       req.Target.Method,
		TargetRPS:          req.TargetRPS,
		DurationS:          req.DurationS,
		Connections:        req.Connections,
		Keepalive:          keepalive,
		TimeoutMS:          timeout,
		ExpectedStatus:     expected,
		MaxLatencyUS:       a.MaxLatencyUS,
		MinBodyBytes:       a.MinBodyBytes,
		MaxBodyBytes:       a.MaxBodyBytes,
		ContentType:        a.ContentType,
		TargetHeaders:      req.Target.Headers,
		EpochUnixUS:        epochUnixUs,
		ControlTickURL:     fmt.Sprintf("%s/v1/_internal/runs/%s/tick", self, runID),
		ControlFinalizeURL: fmt.Sprintf("%s/v1/_internal/runs/%s/finalize", self, runID),
	})
	if err != nil {
		s.dispatchFailed(runID, "dispatch marshal: "+err.Error())
		return
	}

	url := strings.TrimRight(s.CoordinatorURL, "/") + "/admin/runs"
	ctx, cancel := context.WithTimeout(context.Background(), time.Duration(req.DurationS+120)*time.Second)
	defer cancel()
	httpReq, err := http.NewRequestWithContext(ctx, http.MethodPost, url, bytes.NewReader(payload))
	if err != nil {
		s.dispatchFailed(runID, "dispatch build: "+err.Error())
		return
	}
	httpReq.Header.Set("Content-Type", "application/json")

	resp, err := s.dispatchClient.Do(httpReq)
	if err != nil {
		s.dispatchFailed(runID, "coordinator unreachable: "+err.Error())
		return
	}
	defer resp.Body.Close()
	if resp.StatusCode >= 300 {
		s.dispatchFailed(runID, fmt.Sprintf("coordinator returned HTTP %d", resp.StatusCode))
		return
	}
	// Success: the coordinator's finalize push already set the terminal status.
	s.Log.Info("run dispatched", "run_id", runID, "target_rps", req.TargetRPS, "duration_s", req.DurationS)
}

func (s *Server) dispatchFailed(runID, reason string) {
	s.Log.Warn("run dispatch failed", "run_id", runID, "reason", reason)
	if err := s.Store.MarkFailed(context.Background(), runID, reason); err != nil {
		s.Log.Warn("mark failed", "run_id", runID, "err", err.Error())
	}
}

// abortOnCoordinator asks the coordinator to stop the in-flight fleet for a run.
// Best-effort and bounded: the control-plane DB status is already authoritative;
// this propagates the stop to the workers (<1s). No-op if dispatch is disabled.
func (s *Server) abortOnCoordinator(runID string) {
	if s.CoordinatorURL == "" {
		return
	}
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	url := strings.TrimRight(s.CoordinatorURL, "/") + "/admin/runs/" + runID + "/abort"
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, url, nil)
	if err != nil {
		s.Log.Warn("abort build", "run_id", runID, "err", err.Error())
		return
	}
	resp, err := s.dispatchClient.Do(req)
	if err != nil {
		s.Log.Warn("coordinator abort unreachable", "run_id", runID, "err", err.Error())
		return
	}
	_ = resp.Body.Close()
}

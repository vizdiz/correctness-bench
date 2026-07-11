#!/usr/bin/env bash
# Gate #3 - fleet merge correctness. Two workers each at half load must
# aggregate to the same result as one worker at full load. Proves the HDR
# merge across the fleet is lossless and that schedule-sync holds (no clock
# skew silently splitting one 2000-RPS run into two drifting 1000s).
#
# Design: BOTH runs go through the coordinator, differing only in worker
# count. Same dispatch/merge/finalize code path on both sides, so the two
# summaries are apples-to-apples (identical JSON shape from the finalize
# push). Run A = 1 worker @ full RPS. Run B = N workers @ full RPS, the
# coordinator splits it evenly. We capture each run's finalize body with a
# one-shot HTTP sink and compare merged p50/p95/p99 + total count.
#
# The mock is driven in healthy mode (no cliff) so correctness stays 100%
# and we are isolating the latency-merge + rate-sync properties the gate
# actually cares about.
#
# Run:
#   gates/gate3_merge.sh                    # defaults: 2000 RPS, 60s, 2 workers
#   RPS=1000 DURATION_S=20 gates/gate3_merge.sh
#   WORKERS=4 gates/gate3_merge.sh          # Run B fans across 4 workers
#
# Exits 0 on agreement, 1 on any tolerance miss, 2 on a setup error.
set -euo pipefail
cd "$(dirname "$0")/.."

RPS="${RPS:-2000}"
DURATION_S="${DURATION_S:-60}"
WORKERS="${WORKERS:-2}"                     # worker count for Run B
CONNS="${CONNS:-50}"                        # total connections (split across workers)
BASE_LATENCY_MS="${BASE_LATENCY_MS:-10}"
LATENCY_TOL_PCT="${LATENCY_TOL_PCT:-5}"     # p50/p95/p99 tolerance
COUNT_TOL_PCT="${COUNT_TOL_PCT:-2}"         # total request count tolerance
RATE_TOL_PCT="${RATE_TOL_PCT:-10}"          # effective_rps vs target tolerance

MOCK_PORT="${MOCK_PORT:-9092}"
COORD_GRPC_PORT="${COORD_GRPC_PORT:-9190}"
COORD_ADMIN_PORT="${COORD_ADMIN_PORT:-9191}"
WORKER_PORT_BASE="${WORKER_PORT_BASE:-9200}"   # worker i listens on BASE+i
SINK_PORT="${SINK_PORT:-9099}"

RESULTS_DIR="${RESULTS_DIR:-gates/results}"
ARTIFACT="$RESULTS_DIR/gate3.json"

if ! command -v cargo >/dev/null 2>&1; then
  echo "gate3: cargo not on PATH" >&2
  exit 2
fi
if ! command -v python3 >/dev/null 2>&1; then
  echo "gate3: python3 not on PATH (needed for the finalize sink + compare)" >&2
  exit 2
fi

echo "==> building mock + coordinator + worker_node (release)..."
build_log=$(mktemp)
(cd mock && cargo build --release --bin mock) >"$build_log" 2>&1
(cd engine && cargo build --release --bin coordinator --bin worker_node) >>"$build_log" 2>&1 || {
  echo "gate3: build failed" >&2; tail -30 "$build_log" >&2; exit 2;
}

mock_bin=mock/target/release/mock
coord_bin=engine/target/release/coordinator
worker_bin=engine/target/release/worker_node

# --- process bookkeeping: everything we spawn gets killed on exit ----------
declare -a PIDS=()
cleanup() {
  for p in "${PIDS[@]:-}"; do kill "$p" >/dev/null 2>&1 || true; done
  rm -f "$build_log"
}
trap cleanup EXIT

echo "==> starting mock on :$MOCK_PORT (healthy, base_latency_ms=$BASE_LATENCY_MS)..."
MOCK_ADDR="127.0.0.1:$MOCK_PORT" "$mock_bin" >/tmp/gate3_mock.log 2>&1 &
PIDS+=("$!")
for _ in $(seq 1 20); do
  if curl -sf "http://127.0.0.1:$MOCK_PORT/api?mode=healthy&base_latency_ms=1" -o /dev/null; then break; fi
  sleep 0.2
done

TARGET="http://127.0.0.1:$MOCK_PORT/api?mode=healthy&base_latency_ms=$BASE_LATENCY_MS"

# One-shot HTTP sink: accepts a single POST, writes the JSON body to $1, 200s,
# exits. The coordinator sends exactly one finalize POST per run.
sink_py=$(mktemp --suffix=.py 2>/dev/null || mktemp)
cat >"$sink_py" <<'PY'
import sys, http.server
out_path, port = sys.argv[1], int(sys.argv[2])
class H(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        n = int(self.headers.get("Content-Length", 0))
        body = self.rfile.read(n)
        with open(out_path, "wb") as f:
            f.write(body)
        self.send_response(200); self.end_headers()
        self.wfile.write(b"ok")
        # Shut the server down after this one request.
        import threading; threading.Thread(target=self.server.shutdown).start()
    def log_message(self, *a): pass
http.server.HTTPServer(("127.0.0.1", port), H).serve_forever()
PY

# run_fleet <n_workers> <out_json> : bring up a coordinator + n worker_nodes,
# dispatch one run at $RPS for $DURATION_S, capture the finalize body.
run_fleet() {
  local n="$1" out="$2"
  local grpc="127.0.0.1:$COORD_GRPC_PORT" admin="127.0.0.1:$COORD_ADMIN_PORT"
  local sink_out; sink_out=$(mktemp)
  local -a local_pids=()

  # finalize sink
  python3 "$sink_py" "$sink_out" "$SINK_PORT" >/dev/null 2>&1 &
  local sink_pid=$!; local_pids+=("$sink_pid"); PIDS+=("$sink_pid")

  # coordinator
  COORD_GRPC_ADDR="$grpc" COORD_ADMIN_ADDR="$admin" "$coord_bin" \
    >/tmp/gate3_coord.log 2>&1 &
  local coord_pid=$!; local_pids+=("$coord_pid"); PIDS+=("$coord_pid")
  for _ in $(seq 1 30); do
    curl -sf "http://$admin/healthz" >/dev/null 2>&1 && break; sleep 0.2
  done

  # n worker_nodes
  local i
  for i in $(seq 1 "$n"); do
    local wport=$((WORKER_PORT_BASE + i))
    COORD_URL="http://$grpc" \
    WORKER_LISTEN="127.0.0.1:$wport" \
    WORKER_ADDRESS="127.0.0.1:$wport" \
    WORKER_ID="gate3-w$i" \
      "$worker_bin" >/tmp/gate3_worker_$i.log 2>&1 &
    local wp=$!; local_pids+=("$wp"); PIDS+=("$wp")
  done

  # wait until all n workers are registered
  local registered=0
  for _ in $(seq 1 50); do
    registered=$(curl -sf "http://$admin/admin/workers" 2>/dev/null \
      | python3 -c 'import sys,json;print(len(json.load(sys.stdin)))' 2>/dev/null || echo 0)
    [[ "$registered" == "$n" ]] && break
    sleep 0.2
  done
  if [[ "$registered" != "$n" ]]; then
    echo "gate3: only $registered/$n workers registered" >&2
    for p in "${local_pids[@]}"; do kill "$p" >/dev/null 2>&1 || true; done
    return 2
  fi

  # dispatch. /admin/runs blocks until the run finishes, then the coordinator
  # pushes the finalize body to our sink.
  curl -sf -X POST "http://$admin/admin/runs" \
    -H 'content-type: application/json' \
    -d "{\"run_id\":\"gate3-n$n\",\"target_url\":\"$TARGET\",\"target_rps\":$RPS,\"duration_s\":$DURATION_S,\"connections\":$CONNS,\"expected_status\":[200],\"control_finalize_url\":\"http://127.0.0.1:$SINK_PORT/finalize\"}" \
    >/tmp/gate3_dispatch_n$n.json 2>&1 || {
      echo "gate3: dispatch (n=$n) failed" >&2
      for p in "${local_pids[@]}"; do kill "$p" >/dev/null 2>&1 || true; done
      return 2
    }

  # give the finalize sink a moment to land the body
  for _ in $(seq 1 25); do [[ -s "$sink_out" ]] && break; sleep 0.2; done

  # tear down this fleet before the next run reuses the ports
  for p in "${local_pids[@]}"; do kill "$p" >/dev/null 2>&1 || true; done
  sleep 0.5

  if [[ ! -s "$sink_out" ]]; then
    echo "gate3: no finalize body captured for n=$n" >&2
    return 2
  fi
  cp "$sink_out" "$out"
  rm -f "$sink_out"
}

full_json=$(mktemp)
half_json=$(mktemp)

echo "==> Run A: 1 worker @ ${RPS} RPS for ${DURATION_S}s"
run_fleet 1 "$full_json" || exit 2
echo "==> Run B: ${WORKERS} workers @ ${RPS} RPS (split) for ${DURATION_S}s"
run_fleet "$WORKERS" "$half_json" || exit 2

mkdir -p "$RESULTS_DIR"

# Compare + emit the artifact in one python pass. Exit code 0 = PASS, 1 = FAIL.
GENERATED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
set +e
python3 - "$full_json" "$half_json" "$ARTIFACT" <<PY
import json, sys
full = json.load(open(sys.argv[1]))
half = json.load(open(sys.argv[2]))
artifact_path = sys.argv[3]

lat_tol   = float("$LATENCY_TOL_PCT")
count_tol = float("$COUNT_TOL_PCT")
rate_tol  = float("$RATE_TOL_PCT")
target    = float("$RPS")

def pct(base, cand):
    if not base: return 0.0
    return abs(cand - base) / base * 100.0

checks = []
def check(name, delta, tol):
    ok = delta <= tol
    checks.append((name, round(delta, 3), tol, ok))
    return ok

for m in ("p50_us", "p95_us", "p99_us"):
    check(m, pct(full[m], half[m]), lat_tol)
check("total_requests", pct(full["total_requests"], half["total_requests"]), count_tol)
# Run B's synced global offered rate must land near target (proves no drift).
check("effective_rps_vs_target", pct(target, half.get("effective_rps", 0.0)), rate_tol)

verdict = "PASS" if all(ok for *_, ok in checks) else "FAIL"

artifact = {
  "gate": 3,
  "name": "fleet merge correctness",
  "verdict": verdict,
  "generated_at": "$GENERATED_AT",
  "params": {
    "target_rps": target, "duration_s": int("$DURATION_S"),
    "workers_run_b": int("$WORKERS"), "connections": int("$CONNS"),
    "base_latency_ms": int("$BASE_LATENCY_MS"),
  },
  "tolerances_pct": {"latency": lat_tol, "count": count_tol, "rate": rate_tol},
  "full": {k: full.get(k) for k in ("p50_us","p95_us","p99_us","total_requests","total_pass","effective_rps")},
  "half_plus_half": {k: half.get(k) for k in ("p50_us","p95_us","p99_us","total_requests","total_pass","effective_rps")},
  "checks": [{"metric": n, "delta_pct": d, "tol_pct": t, "pass": ok} for n, d, t, ok in checks],
}
json.dump(artifact, open(artifact_path, "w"), indent=2)
open(artifact_path, "a").write("\n")

print()
print(f"{'metric':<26} {'full':>12} {'half+half':>12} {'delta%':>8} {'tol%':>6}  result")
for k in ("p50_us","p95_us","p99_us","total_requests"):
    print(f"{k:<26} {full.get(k,'-'):>12} {half.get(k,'-'):>12} "
          f"{pct(full.get(k,0), half.get(k,0)):>8.2f} "
          f"{(lat_tol if k!='total_requests' else count_tol):>6.1f}  "
          f"{'PASS' if [c for c in checks if c[0]==k][0][3] else 'FAIL'}")
erps = half.get("effective_rps", 0.0)
print(f"{'effective_rps (Run B)':<26} {'':>12} {erps:>12.1f} "
      f"{pct(target, erps):>8.2f} {rate_tol:>6.1f}  "
      f"{'PASS' if [c for c in checks if c[0]=='effective_rps_vs_target'][0][3] else 'FAIL'}")
print()
print(f"==> GATE #3 {verdict}  (artifact: {artifact_path})")
sys.exit(0 if verdict == "PASS" else 1)
PY
rc=$?
set -e
rm -f "$full_json" "$half_json" "$sink_py"
exit $rc

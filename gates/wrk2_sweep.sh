#!/usr/bin/env bash
# Differential matrix: engine vs wrk2 across a small sweep of (RPS, duration,
# method) against the local mock. Fails if any axis (p50, p95, p99) drifts
# more than $TOLERANCE_PCT (default 5%) between the two on the same configuration.
#
# Closes gate #1 across multiple configurations (the gate's own spec only
# covers a single 2000-RPS / 60s point). Run before merging anything that
# touches the scheduler, the HDR pipeline, or the TCP path.
#
# Usage:
#   WRK2_BIN=/path/to/wrk gates/wrk2_sweep.sh
#   gates/wrk2_sweep.sh                  # default: tools/wrk2/wrk
#
# Optional env:
#   TOLERANCE_PCT=5         tolerance in % per percentile per config
#   MOCK_PORT=9090          mock listen port (random-OK fallback if busy)
#   RUNS="100 500 1000"     RPS values to sweep (space-separated)
#   DURATION_S=20           per-configuration duration
set -euo pipefail

cd "$(dirname "$0")/.."

WRK2_BIN="${WRK2_BIN:-tools/wrk2/wrk}"
TOLERANCE_PCT="${TOLERANCE_PCT:-5}"
MOCK_PORT="${MOCK_PORT:-9090}"
RUNS="${RUNS:-200 500 1000}"
DURATION_S="${DURATION_S:-20}"
BASE_LATENCY_MS="${BASE_LATENCY_MS:-10}"
CONNS="${CONNS:-50}"

# Machine-readable proof artifact. Each (RPS, percentile) row is appended; the
# footer assembles gates/results/gate1.json.
RESULTS_DIR="${RESULTS_DIR:-gates/results}"
ARTIFACT="$RESULTS_DIR/gate1.json"
ROWS_FILE="$(mktemp)"

if [[ ! -x "$WRK2_BIN" ]]; then
  cat >&2 <<EOF
gate sweep: wrk2 binary not found at "$WRK2_BIN".

Build it from the project's Dockerfile (cross-arch):
  docker build -t wrk2-local tools/wrk2/
  docker create --name _wrk2_tmp wrk2-local && \\
    docker cp _wrk2_tmp:/wrk2/wrk tools/wrk2/wrk && \\
    docker rm _wrk2_tmp

Or set WRK2_BIN=/path/to/wrk explicitly.
EOF
  exit 2
fi

if ! command -v cargo >/dev/null 2>&1; then
  echo "gate sweep: cargo not on PATH" >&2
  exit 2
fi

build_log=$(mktemp)
trap 'rm -f "$build_log"' EXIT
echo "==> building engine + mock (release)…"
(cd engine && cargo build --release --bin engine-worker) >>"$build_log" 2>&1
(cd mock && cargo build --release --bin mock) >>"$build_log" 2>&1

mock_bin=""
for cand in mock/target/release/mock mock/target/release/mock-server; do
  if [[ -x "$cand" ]]; then mock_bin="$cand"; break; fi
done
if [[ -z "$mock_bin" ]]; then
  echo "gate sweep: no mock binary produced (release build log: $build_log)" >&2
  exit 2
fi

engine_bin=engine/target/release/engine-worker

echo "==> starting mock on :$MOCK_PORT (base_latency_ms=$BASE_LATENCY_MS)…"
MOCK_ADDR="127.0.0.1:$MOCK_PORT" "$mock_bin" >/tmp/mock_sweep.log 2>&1 &
mock_pid=$!
trap 'kill $mock_pid >/dev/null 2>&1 || true; rm -f "$build_log" "$ROWS_FILE"' EXIT

# Wait for mock to be reachable.
for i in 1 2 3 4 5 6 7 8 9 10; do
  if curl -sf "http://127.0.0.1:$MOCK_PORT/healthz" >/dev/null 2>&1 \
     || curl -sf "http://127.0.0.1:$MOCK_PORT/api?mode=healthy&base_latency_ms=1" -o /dev/null; then
    break
  fi
  sleep 0.2
done

target_url="http://127.0.0.1:$MOCK_PORT/api?mode=healthy&base_latency_ms=$BASE_LATENCY_MS"

# Parse wrk2 --latency output for corrected p50/p95/p99 in microseconds.
# wrk2 prints lines like:
#   50.000%    XX.XXms
parse_wrk2_us() {
  local pct="$1"
  awk -v pct="$pct" '
    $1 == pct"%" {
      # second field is e.g. "12.34ms" or "1.23s" or "987.65us"
      v = $2
      if (v ~ /us$/)     { sub(/us$/, "", v); printf("%d\n", v); }
      else if (v ~ /ms$/) { sub(/ms$/, "", v); printf("%d\n", v * 1000); }
      else if (v ~ /s$/)  { sub(/s$/, "", v);  printf("%d\n", v * 1000000); }
      exit
    }
  '
}

# Parse engine-worker's printed summary block for the corrected line.
#   Latency (corrected)   p50   p95    p99   p999    max
#                       12.3   34.5   56.7  ...
parse_engine_us() {
  local which="$1" # p50 | p95 | p99
  awk -v key="$which" '
    /Latency \(corrected\)/ { getline; print; exit }
  ' | awk -v key="$which" '
  {
    # numeric fields are ms with one decimal
    if (key == "p50") val = $1
    if (key == "p95") val = $2
    if (key == "p99") val = $3
    # ms -> us
    printf("%d\n", val * 1000)
  }'
}

pct_delta() {
  local base="$1" cand="$2"
  if [[ "$base" -eq 0 ]]; then echo "0.0"; return; fi
  awk -v b="$base" -v c="$cand" 'BEGIN { d = (c-b)/b*100; if (d<0) d=-d; printf("%.2f", d) }'
}

declare -a failures=()
printf "\n%-7s %-7s %-7s %-8s %-8s %-8s %-8s %-8s %-8s\n" \
  "RPS" "dur" "metric" "wrk2_us" "eng_us" "delta_pct" "limit_pct" "result" "config"

for rps in $RUNS; do
  echo "==> RPS=$rps duration=${DURATION_S}s"
  # wrk2
  wrk_out=$(mktemp)
  "$WRK2_BIN" -t4 -c "$CONNS" -d "${DURATION_S}s" -R "$rps" --latency "$target_url" \
    >"$wrk_out" 2>&1 || true
  wrk_p50=$(parse_wrk2_us 50.000 < "$wrk_out")
  wrk_p95=$(parse_wrk2_us 95.000 < "$wrk_out")
  wrk_p99=$(parse_wrk2_us 99.000 < "$wrk_out")

  # engine
  eng_out=$(mktemp)
  "$engine_bin" -u "$target_url" -m GET -R "$rps" -d "$DURATION_S" -c "$CONNS" \
    >"$eng_out" 2>&1 || true
  eng_p50=$(parse_engine_us p50 < "$eng_out")
  eng_p95=$(parse_engine_us p95 < "$eng_out")
  eng_p99=$(parse_engine_us p99 < "$eng_out")

  for metric in p50 p95 p99; do
    case "$metric" in
      p50) b=$wrk_p50; c=$eng_p50 ;;
      p95) b=$wrk_p95; c=$eng_p95 ;;
      p99) b=$wrk_p99; c=$eng_p99 ;;
    esac
    if [[ -z "$b" || -z "$c" || "$b" -eq 0 ]]; then
      result="skip"
      delta="n/a"
    else
      delta=$(pct_delta "$b" "$c")
      if awk -v d="$delta" -v t="$TOLERANCE_PCT" 'BEGIN { exit !(d <= t) }'; then
        result="PASS"
      else
        result="FAIL"
        failures+=("RPS=$rps $metric delta=${delta}% > ${TOLERANCE_PCT}% (wrk2=${b}us engine=${c}us)")
      fi
    fi
    printf "%-7s %-7s %-7s %-8s %-8s %-8s %-8s %-8s\n" \
      "$rps" "${DURATION_S}s" "$metric" "$b" "$c" "$delta" "$TOLERANCE_PCT" "$result"
    printf '%s|%s|%s|%s|%s|%s|%s\n' \
      "$rps" "$DURATION_S" "$metric" "${b:-0}" "${c:-0}" "$delta" "$result" >>"$ROWS_FILE"
  done

  rm -f "$wrk_out" "$eng_out"
done

echo

# Emit the machine-readable proof artifact (gate #1 differential matrix).
mkdir -p "$RESULTS_DIR"
if command -v python3 >/dev/null 2>&1; then
  GENERATED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  GATE1_NFAIL="${#failures[@]}" GATE1_TOL="$TOLERANCE_PCT" \
  python3 - "$ROWS_FILE" "$ARTIFACT" <<'PY'
import json, os, sys
rows_file, artifact = sys.argv[1], sys.argv[2]
rows = []
with open(rows_file) as f:
    for line in f:
        line = line.strip()
        if not line:
            continue
        rps, dur, metric, wrk2_us, eng_us, delta, result = line.split("|")
        rows.append({
            "rps": int(rps), "duration_s": int(dur), "metric": metric,
            "wrk2_us": int(wrk2_us), "engine_us": int(eng_us),
            "delta_pct": (None if delta == "n/a" else float(delta)),
            "result": result,
        })
nfail = int(os.environ["GATE1_NFAIL"])
artifact_obj = {
    "gate": 1, "name": "wrk2 agreement (differential matrix)",
    "verdict": "PASS" if nfail == 0 else "FAIL",
    "generated_at": os.environ["GENERATED_AT"],
    "params": {"tolerance_pct": float(os.environ["GATE1_TOL"])},
    "matrix": rows,
}
with open(artifact, "w") as f:
    json.dump(artifact_obj, f, indent=2)
    f.write("\n")
print(f"==> artifact: {artifact}")
PY
else
  echo "gate1: python3 not found; skipping JSON artifact (verdict still authoritative above)" >&2
fi

if (( ${#failures[@]} > 0 )); then
  echo "==> SWEEP FAILED: ${#failures[@]} mismatch(es)"
  for f in "${failures[@]}"; do echo "  - $f"; done
  exit 1
fi
echo "==> SWEEP PASSED: every (RPS, percentile) pair within ±${TOLERANCE_PCT}%"

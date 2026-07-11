#!/usr/bin/env bash
# Gate #2 - the oracle. Injects a known failure rate into the mock and verifies
# the engine reports the same rate within tolerance. Closes the thesis claim:
# "the tool measures correctness, not just throughput."
#
# Modes covered out of the box:
#   fast500    - HTTP 500 above the cliff, fast latency. The headline case:
#                latency stays flat while correctness collapses.
#   slow_ok    - 200 above the cliff but with high latency. fail_latency
#                fires; status-tier correctness stays high.
#   truncate   - body shrinks above the cliff. fail_size fires.
#
# Run:
#   gates/gate2_oracle.sh                          # full suite
#   MODES=fast500 gates/gate2_oracle.sh            # subset
#   TOLERANCE_PCT=2 gates/gate2_oracle.sh          # tighter
#
# Exits 0 on agreement, 1 on any mode failing, 2 on a setup error.
set -euo pipefail
cd "$(dirname "$0")/.."

TOLERANCE_PCT="${TOLERANCE_PCT:-5}"
MOCK_PORT="${MOCK_PORT:-9091}"
DURATION_S="${DURATION_S:-12}"
RPS="${RPS:-400}"
CLIFF_RPS="${CLIFF_RPS:-100}"
INJECT_PCT="${INJECT_PCT:-100}"
MAX_LATENCY_MS="${MAX_LATENCY_MS:-200}"
# truncate's body is ~42 bytes; healthy is ~47. 45 sits between, so size-tier
# fires on truncate and stays clean on healthy.
MIN_BODY="${MIN_BODY:-45}"
MODES="${MODES:-fast500 slow_ok truncate}"
# Floor for how much of the run lands above the cliff. With RPS=400 and a 12s
# duration, the mock's running-average crosses 100 RPS within the first second
# and stays there - so the "above-cliff fraction" is ~91%. Lower bound is set
# generously so a 2-3 second warm-up doesn't fail the gate.
ABOVE_FLOOR_PCT="${ABOVE_FLOOR_PCT:-85}"

# Machine-readable proof artifact. Each mode appends a pipe-delimited row; the
# footer assembles them into gates/results/gate2.json.
RESULTS_DIR="${RESULTS_DIR:-gates/results}"
ARTIFACT="$RESULTS_DIR/gate2.json"
ROWS_FILE="$(mktemp)"

if ! command -v cargo >/dev/null 2>&1; then
  echo "gate sweep: cargo not on PATH" >&2
  exit 2
fi

echo "==> building engine + mock (release)..."
(cd engine && cargo build --release --bin engine-worker) >/tmp/gate2_build.log 2>&1
(cd mock && cargo build --release --bin mock) >>/tmp/gate2_build.log 2>&1

engine=engine/target/release/engine-worker
mock=mock/target/release/mock
echo "==> starting mock on :$MOCK_PORT..."
MOCK_ADDR="127.0.0.1:$MOCK_PORT" "$mock" >/tmp/gate2_mock.log 2>&1 &
mock_pid=$!
trap 'kill $mock_pid >/dev/null 2>&1 || true; rm -f "$ROWS_FILE"' EXIT

for _ in 1 2 3 4 5 6 7 8 9 10; do
  if curl -sf "http://127.0.0.1:$MOCK_PORT/api?mode=healthy&base_latency_ms=1" -o /dev/null; then
    break
  fi
  sleep 0.2
done

# Extract the tier counts the engine prints in its trailing summary block.
parse_metric() {
  local key="$1" out="$2"
  case "$key" in
    completed) awk '/requests/ && /achieved/ {print $2; exit}' < "$out" ;;
    pass)
      awk '/Correctness/ {
        # line is "  Correctness        N / D pass  (X%)  (expected status: ...)"
        for (i=1; i<=NF; i++) if ($i == "/") { print $(i-1); exit }
      }' < "$out"
      ;;
    fail_status)
      awk '/Fail by tier/ {
        for (i=1; i<=NF; i++) if ($i ~ /status=[0-9]+/) {
          sub("status=", "", $i); print $i; exit
        }
      }' < "$out"
      ;;
    fail_latency)
      awk '/Fail by tier/ {
        for (i=1; i<=NF; i++) if ($i ~ /latency=[0-9]+/) {
          sub("latency=", "", $i); print $i; exit
        }
      }' < "$out"
      ;;
    fail_size)
      awk '/Fail by tier/ {
        for (i=1; i<=NF; i++) if ($i ~ /size=[0-9]+/) {
          sub("size=", "", $i); print $i; exit
        }
      }' < "$out"
      ;;
  esac
}

ratio_delta() {
  local injected="$1" reported="$2"
  awk -v a="$injected" -v b="$reported" 'BEGIN { d=b-a; if(d<0)d=-d; printf("%.2f", d) }'
}

run_mode() {
  local mode="$1" expected_metric="$2" expected_pct="$3" engine_flags="$4"
  local target="http://127.0.0.1:$MOCK_PORT/api?mode=$mode&cliff_rps=$CLIFF_RPS&pct=$INJECT_PCT&base_latency_ms=10"
  local out; out=$(mktemp)
  # shellcheck disable=SC2086
  "$engine" -u "$target" -m GET -R "$RPS" -d "$DURATION_S" -c 30 $engine_flags --expected-status 200 \
    >"$out" 2>&1 || true

  local completed pass fail_status fail_latency fail_size
  completed=$(parse_metric completed "$out")
  pass=$(parse_metric pass "$out")
  fail_status=$(parse_metric fail_status "$out")
  fail_latency=$(parse_metric fail_latency "$out")
  fail_size=$(parse_metric fail_size "$out")
  local reported=0
  case "$expected_metric" in
    fail_status)  reported="$fail_status" ;;
    fail_latency) reported="$fail_latency" ;;
    fail_size)    reported="$fail_size" ;;
  esac

  # Inject is on whenever measured RPS > cliff_rps. With RPS=400, cliff=100,
  # nearly the whole run is above-cliff. The expected reported fail rate is
  # INJECT_PCT * above_floor (lower bound on the above-cliff fraction).
  local lower_bound
  lower_bound=$(awk -v p="$expected_pct" -v f="$ABOVE_FLOOR_PCT" \
    'BEGIN { printf("%.2f", p * f / 100.0) }')
  local upper_bound="$expected_pct"
  local reported_pct
  if [[ "${completed:-0}" -gt 0 ]]; then
    reported_pct=$(awk -v r="$reported" -v t="$completed" 'BEGIN { printf("%.2f", r/t*100) }')
  else
    reported_pct="0.00"
  fi
  local verdict
  if awk -v r="$reported_pct" -v lo="$lower_bound" -v hi="$upper_bound" -v tol="$TOLERANCE_PCT" \
       'BEGIN { exit !(r >= lo - tol && r <= hi + tol) }'; then
    verdict="PASS"
  else
    verdict="FAIL"
  fi
  printf "%-10s metric=%-12s reported=%6.2f%%  band=[%.2f,%.2f]%% (±%spp)  (%s)\n" \
    "$mode" "$expected_metric" "$reported_pct" "$lower_bound" "$upper_bound" \
    "$TOLERANCE_PCT" "$verdict"
  printf '%s|%s|%s|%s|%s|%s|%s\n' \
    "$mode" "$expected_metric" "$reported_pct" "$lower_bound" "$upper_bound" \
    "$TOLERANCE_PCT" "$verdict" >>"$ROWS_FILE"
  rm -f "$out"
  if [[ "$verdict" == "FAIL" ]]; then
    return 1
  fi
}

echo
printf "%-10s %-46s %-9s\n" "mode" "metric / expected / reported / delta" "verdict"

failures=0
for mode in $MODES; do
  case "$mode" in
    fast500)
      run_mode "fast500" fail_status "$INJECT_PCT" "" || failures=$((failures+1))
      ;;
    slow_ok)
      # slow_ok injects a slow 200; engine flags --max-latency-ms catches it.
      run_mode "slow_ok" fail_latency "$INJECT_PCT" "--max-latency-ms $MAX_LATENCY_MS" || failures=$((failures+1))
      ;;
    truncate)
      # truncate shrinks the body; min-body-bytes catches it.
      run_mode "truncate" fail_size "$INJECT_PCT" "--min-body-bytes $MIN_BODY" || failures=$((failures+1))
      ;;
    *)
      echo "gate2: unknown mode '$mode'" >&2
      exit 2
      ;;
  esac
done

echo

# Emit the machine-readable proof artifact.
mkdir -p "$RESULTS_DIR"
if command -v python3 >/dev/null 2>&1; then
  GENERATED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  GATE2_FAILURES="$failures" GATE2_TOL="$TOLERANCE_PCT" GATE2_INJECT="$INJECT_PCT" \
  python3 - "$ROWS_FILE" "$ARTIFACT" <<'PY'
import json, os, sys
rows_file, artifact = sys.argv[1], sys.argv[2]
rows = []
with open(rows_file) as f:
    for line in f:
        line = line.strip()
        if not line:
            continue
        mode, metric, reported, lo, hi, tol, verdict = line.split("|")
        rows.append({
            "mode": mode, "metric": metric,
            "reported_pct": float(reported),
            "band_pct": [float(lo), float(hi)],
            "tol_pp": float(tol), "verdict": verdict,
        })
failures = int(os.environ["GATE2_FAILURES"])
artifact_obj = {
    "gate": 2, "name": "oracle (injected failure rate)",
    "verdict": "PASS" if failures == 0 else "FAIL",
    "generated_at": os.environ["GENERATED_AT"],
    "params": {"inject_pct": float(os.environ["GATE2_INJECT"]),
               "tolerance_pp": float(os.environ["GATE2_TOL"])},
    "modes": rows,
}
with open(artifact, "w") as f:
    json.dump(artifact_obj, f, indent=2)
    f.write("\n")
print(f"==> artifact: {artifact}")
PY
else
  echo "gate2: python3 not found; skipping JSON artifact (verdict still authoritative above)" >&2
fi

if (( failures > 0 )); then
  echo "==> GATE #2 FAILED: $failures mode(s) outside tolerance (±${TOLERANCE_PCT}pp)"
  exit 1
fi
echo "==> GATE #2 PASSED: every mode within ±${TOLERANCE_PCT}pp"

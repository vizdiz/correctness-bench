#!/bin/sh
# Gate #1 (wrk2 agreement) run entirely in containers on one docker network:
# wrk2, engine-worker, and the mock all talk container-to-container, so there
# is no Docker Desktop VM-boundary latency skewing one side vs the other.
#
# This is the macOS/arm64 path. Native wrk2 does not build cleanly there (its
# vendored LuaJIT bytecode does not embed, so `wrk` runs but prints nothing);
# on a Linux host, prefer the native `gates/wrk2_sweep.sh`.
#
# Uses only the prebuilt compose images:
#   correctness-bench-wrk2, correctness-bench-engine, correctness-bench-mock
# Build them first if missing:  docker compose build mock coordinator
#                               docker build -t correctness-bench-wrk2 tools/wrk2
#
# Compares the two percentiles both tools emit: p50 and p99. (wrk2 --latency
# prints 50/75/90/99, not 95.) Matches gate1.md's conditions by default:
# base_latency_ms=20, connections=100. Writes gates/results/gate1.json.
#
# Run:   gates/gate1_containers.sh
#        RUNS="2000" DURATION_S=60 gates/gate1_containers.sh   # single spec point
#
# Exits 0 on agreement (<= TOL% on every p50/p99), 1 on drift, 2 on setup error.
set -eu
cd "$(dirname "$0")/.."

RUNS="${RUNS:-500 1000 2000}"
DURATION_S="${DURATION_S:-45}"
CONNS="${CONNS:-100}"
TOL="${TOL:-5}"
BASE_MS="${BASE_MS:-20}"
OUT="${OUT:-gates/results}"
ARTIFACT="$OUT/gate1.json"
NET=gate1net
MOCK=gate1mock
mkdir -p "$OUT"
WORK=$(mktemp -d)

for img in correctness-bench-wrk2 correctness-bench-engine correctness-bench-mock; do
  if ! docker image inspect "$img" >/dev/null 2>&1; then
    echo "gate1: missing image '$img'. Build the compose images + wrk2 first." >&2
    exit 2
  fi
done

cleanup() {
  docker rm -f "$MOCK" >/dev/null 2>&1 || true
  docker network rm "$NET" >/dev/null 2>&1 || true
  rm -rf "$WORK"
}
trap cleanup EXIT

echo "==> network + mock (base_latency_ms=$BASE_MS)"
docker network create "$NET" >/dev/null 2>&1 || true
docker rm -f "$MOCK" >/dev/null 2>&1 || true
docker run -d --name "$MOCK" --network "$NET" -e MOCK_ADDR=0.0.0.0:8080 \
  correctness-bench-mock >/dev/null
for _ in $(seq 1 30); do
  if docker run --rm --network "$NET" correctness-bench-wrk2 \
       -t1 -c1 -d1s -R1 "http://$MOCK:8080/api?mode=healthy&base_latency_ms=1" >/dev/null 2>&1; then
    break
  fi
  sleep 0.3
done

TARGET="http://$MOCK:8080/api?mode=healthy&base_latency_ms=$BASE_MS"
: > "$WORK/rows"
for rps in $RUNS; do
  echo "==> RPS=$rps dur=${DURATION_S}s"
  docker run --rm --network "$NET" correctness-bench-engine \
    -u "$TARGET" -m GET -R "$rps" -d "$DURATION_S" -c "$CONNS" \
    >"$WORK/eng_$rps.txt" 2>&1 || true
  docker run --rm --network "$NET" correctness-bench-wrk2 \
    -t4 -c "$CONNS" -d "${DURATION_S}s" -R "$rps" --latency "$TARGET" \
    >"$WORK/wrk_$rps.txt" 2>&1 || true
  echo "$rps" >> "$WORK/rows"
done

echo "==> compare + artifact"
GENERATED_AT="$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
DURATION_S="$DURATION_S" TOL="$TOL" BASE_MS="$BASE_MS" CONNS="$CONNS" \
python3 - "$WORK" "$ARTIFACT" <<'PY'
import json, os, re, sys
work, artifact = sys.argv[1], sys.argv[2]
tol = float(os.environ["TOL"]); dur = int(os.environ["DURATION_S"])

def eng_us(txt, which):
    m = re.search(r"Latency \(corrected\).*\n\s*([\d.]+)\s+([\d.]+)\s+([\d.]+)", txt)
    if not m: return None
    return int(float(m.group({"p50": 1, "p99": 3}[which])) * 1000)

def wrk_us(txt, which):
    pct = {"p50": "50.000", "p99": "99.000"}[which]
    m = re.search(rf"^\s*{re.escape(pct)}%\s+([\d.]+)(us|ms|s)\b", txt, re.M)
    if not m: return None
    return int(float(m.group(1)) * {"us": 1, "ms": 1000, "s": 1_000_000}[m.group(2)])

rows, failures = [], 0
for rps in [int(x) for x in open(os.path.join(work, "rows")).read().split()]:
    etxt = open(os.path.join(work, f"eng_{rps}.txt")).read()
    wtxt = open(os.path.join(work, f"wrk_{rps}.txt")).read()
    for metric in ("p50", "p99"):
        e = eng_us(etxt, metric); w = wrk_us(wtxt, metric)
        if not e or not w:
            rows.append({"rps": rps, "duration_s": dur, "metric": metric,
                         "wrk2_us": w, "engine_us": e, "delta_pct": None, "result": "skip"}); continue
        delta = abs(w - e) / w * 100.0
        ok = delta <= tol
        if not ok: failures += 1
        rows.append({"rps": rps, "duration_s": dur, "metric": metric, "wrk2_us": w,
                     "engine_us": e, "delta_pct": round(delta, 3), "result": "PASS" if ok else "FAIL"})

obj = {"gate": 1, "name": "wrk2 agreement (differential matrix, containerized)",
       "verdict": "PASS" if failures == 0 else "FAIL",
       "generated_at": os.environ["GENERATED_AT"],
       "params": {"tolerance_pct": tol, "percentiles": ["p50", "p99"],
                  "base_latency_ms": int(os.environ["BASE_MS"]), "connections": int(os.environ["CONNS"]),
                  "duration_s": dur, "rps_swept": [r["rps"] for r in rows[::2]],
                  "method": "wrk2 + engine-worker + mock as containers on one docker network"},
       "matrix": rows}
json.dump(obj, open(artifact, "w"), indent=2); open(artifact, "a").write("\n")

print(f"\n{'RPS':>6} {'metric':>6} {'wrk2_us':>9} {'engine_us':>10} {'delta%':>8} {'result':>6}")
for r in rows:
    d = ('%.2f' % r['delta_pct']) if r['delta_pct'] is not None else 'n/a'
    print(f"{r['rps']:>6} {r['metric']:>6} {str(r['wrk2_us']):>9} {str(r['engine_us']):>10} {d:>8} {r['result']:>6}")
print(f"\n==> GATE #1 {obj['verdict']}  (artifact: {artifact})")
sys.exit(0 if failures == 0 else 1)
PY

# mock — load-dependent failure-injection target

A real deployable HTTP service whose failure behavior is a function of the
**current offered RPS**, so a *cliff* appears at a threshold instead of as
uniform noise. Everything in the project validates against this. Triple duty:
agreement bench vs wrk2 (gate #1), cliff demo, ground-truth oracle (gate #2).

## Run

```bash
cargo run                          # serves on 127.0.0.1:8080
MOCK_ADDR=0.0.0.0:9000 cargo run   # override bind address
cargo test                         # 6 unit + 7 integration tests
# docker compose up mock           # (port 8080; pending a Docker runtime — see docs/DECISIONS.md D2)
```

Endpoint: `GET|POST /api` · health: `GET /healthz` (cheap, not metered).

## Knobs (query params)

| param | default | meaning |
|-------|---------|---------|
| `mode` | `healthy` | `healthy` \| `fast500` \| `truncate` \| `wrong_value` \| `slow_ok` |
| `cliff_rps` | `100` | injection turns on once the measured rolling-1s RPS **exceeds** this |
| `pct` | `100` | percent of *above-cliff* requests that get the failure (deterministic: `seq % 100 < pct`) |
| `base_latency_ms` | `0` | fixed latency applied to every response |

Below the cliff (or for requests not selected by `pct`), **every mode returns
the healthy response**. `healthy` mode never injects regardless of load. An
unknown `mode` returns `400`.

Response debug headers: `x-mock-mode`, `x-mock-rps`, `x-mock-above-cliff`,
`x-mock-injected`.

### Reproducible cliff control
The rolling meter is real, but for deterministic tests/demos you can pin the
side of the cliff without timing games:
- `cliff_rps=1000000` → every request stays **below** the cliff (always healthy).
- `cliff_rps=0` → every request is **above** the cliff (injection per `pct`).

## Modes — what triggers each, what to observe

### `healthy` — always OK, fixed base latency (gate #1)
```bash
curl -i "localhost:8080/api?mode=healthy&base_latency_ms=20"
# 200, {"status":"ok","count":3,"items":["a","b","c"]}
```
Internal handling < 1 ms on top of `base_latency_ms` (measured ~0.76 ms total
round-trip on loopback at base=0). Must out-run the load generator so gate #1
measures the engine, not the mock.

### `fast500` — fast HTTP 500 above the cliff (gate #2, the headline)
```bash
curl -i "localhost:8080/api?mode=fast500&cliff_rps=100&pct=100"
```
Below 100 RPS: `200` healthy. Above 100 RPS: `200`→`500` at the **same latency**
as healthy. This is the latency-blind case — wrk2 sees flat latency and calls it
a win; correctness collapses. The whole pitch.

### `truncate` — truncated / invalid JSON above the cliff
```bash
curl -i "localhost:8080/api?mode=truncate&cliff_rps=100&pct=100"
# above cliff: 200, body = {"status":"ok","count":3,"items":["a","b   (does NOT parse)
```
Status `200` (status tier passes), but body is short + invalid JSON. Trips the
**size** tier inline and the **schema** tier in the offload pool.

### `wrong_value` — schema-valid but wrong value above the cliff
```bash
curl -i "localhost:8080/api?mode=wrong_value&cliff_rps=100&pct=100"
# above cliff: 200, {"status":"ok","count":-1,"items":["a","b","c"]}
```
Parses fine, has all fields — only the **value** is wrong (`count:-1`). Caught
only by the **value** tier (offload, e.g. a JSON-path assertion `count >= 0`).
Status/size/schema all pass. The case Postman-style checks miss.

### `slow_ok` — correct but late above the cliff
```bash
curl -s -o /dev/null -w "%{http_code} %{time_total}s\n" \
  "localhost:8080/api?mode=slow_ok&cliff_rps=100&pct=100"
# above cliff: 200, correct body, ~+500ms (SLOW_PENALTY_MS)
```
`200` with the **correct** body, but slow (base + 500 ms). Trips the **latency**
tier while correctness stays green — the inverse of `fast500`. Confirms latency
and correctness are independent axes.

## Demo recipe (the cliff in 30 seconds)
1. `mode=healthy&base_latency_ms=20` under rising load → flat latency, 100% correct.
2. Switch to `mode=fast500&cliff_rps=100` and ramp through 100 RPS → latency
   stays flat (right axis), correctness cliffs to 0 at 100 RPS (left axis).
3. `mode=slow_ok` for the inverse: correctness flat, latency cliffs.

## Internals
- `RpsMeter`: 10 × 100 ms buckets, lock-free-ish; sum over the last second ≈ RPS.
- `decide()` is pure (knobs + RPS + sequence → response) and unit-tested.
- Determinism comes from a per-instance request counter, not wall-clock timing.

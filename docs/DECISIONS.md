# Decisions log

Non-obvious choices made during the overnight scaffolding session. Each is
flagged for human review — override any of these freely.

Format: **[ID] decision** — options considered, why, review note.

---

## Environment / tooling

### [D1] Installed Rust toolchain via rustup (stable 1.96.0)
Rust was not present on the machine; the mock and CLI require it. rustup is the
standard, reversible install (`rustup self uninstall` reverts). **Review:** fine
to keep; it's needed for mock/engine/cli.

### [D2] Did NOT install Docker / colima unsupervised
No Docker runtime is present (no Docker Desktop, no colima/podman). Installing a
VM-backed runtime (colima/lima) overnight is a heavier, more opinionated change
to your machine than a language toolchain, so I declined. **Consequence:**
`docker compose up`, the Timescale container, and all container-level
verification are NOT executed this session. All Dockerfiles + compose are
written and ready. **Review / ACTION:** install Docker Desktop (or `brew install
colima docker docker-compose && colima start`) and run the compose verification
steps listed in BUILD_LOG.md.

### [D3] Used `pg-query-emscripten` (libpg_query → wasm) to validate schema.sql
The build kit suggested "libpg_query if available, else syntax-scan." Homebrew's
python3 is broken (pyexpat symbol mismatch) so the Python `pglast` route failed;
`pg-query-emscripten` is the same libpg_query parser compiled to wasm, run under
Node. Faithful to the requirement, no native build needed. **Review:** none
needed; this was a validation-only tool, not a project dependency.

### [D4] Installed `protobuf` (protoc 35.0) via Homebrew
Needed for the `protoc --descriptor_set_out` contract validator. **Review:** keep;
engine work tomorrow needs protoc + a Rust/Go plugin anyway.

---

## Repo / layout

### [D5] Kept Rust crates independent (no top-level Cargo workspace)
The kit listed engine/ as "Cargo workspace" but engine/mock/cli are separate
concerns built by separate agents at different times. Independent crates let
`mock/` build and test in isolation tonight without dragging in the (stubbed)
engine. **Review:** if you'd prefer one workspace later (shared HDR types between
engine + mock), promote to a root `[workspace]` then. Low cost to change.

### [D6] Preserved the build-kit usage guide as `BUILDKIT_README.md`
`build-kit/README.md` was "how to use this kit", not a project README. I moved it
to `BUILDKIT_README.md` and wrote a proper project `README.md` at root. **Review:**
delete `BUILDKIT_README.md` whenever you like; it's historical now.

### [D7] engine/ left as a single stub lib crate
Per the absolute rule "do not touch /engine/", I created only the minimal
`Cargo.toml` + `src/lib.rs` sentinel the setup step asked for. No worker/
coordinator structure pre-created. **Review:** engine agent owns this tomorrow.

---

## Control plane (Phase 3) — see entries added during that phase below

### [D8] Router: chi
(Recorded at implementation time — see BUILD_LOG. chi over stdlib for typed URL
params + middleware ergonomics while staying net/http-compatible and tiny.)

### [D9] Migration tool: goose
goose over atlas: simpler, single binary, plain SQL migrations that wrap the
frozen schema.sql verbatim without a DSL. The migration is embedded (go:embed) and
run programmatically on startup via the pgx stdlib driver. A drift test asserts the
migration matches contracts/schema.sql byte-for-byte (modulo comments/whitespace).

### [D11] `AUTO_MIGRATE` env toggle (default true)
control runs goose on startup by default (convenient for docker-compose), but
`AUTO_MIGRATE=false` skips it for when migrations run as a separate deploy step or
the schema is pre-applied. **Why added:** lets the server boot against an
already-migrated DB; used it to verify the live server against plain Postgres
(goose's verbatim schema needs Timescale, which wasn't installed). **Review:**
keep, or always run migrations as a separate job and default this false.

### [D12] Test-only Timescale shim (no contract/migration fork)
DB tests run against plain Postgres 16 (no Timescale installed — see D2). The test
harness applies the frozen schema with the 3 timescaledb-specific statements
skipped (extension + 2 `create_hypertable` calls), so hypertables become plain
tables. This does NOT touch the frozen contract or the migration, and does NOT
weaken the canary gate: the credential path only writes `runs`, which is identical
either way. The shim asserts each skipped statement still exists verbatim (drift
guard). **Review:** re-run the full suite against Timescale once Docker is up to
confirm the hypertable path; expected to pass unchanged.

---

## Web (Phase 4) — see entries added during that phase below

### [D10] Font + palette
(Recorded at implementation time if Phase 4 is reached.)

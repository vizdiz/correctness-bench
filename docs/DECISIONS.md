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
(Recorded at implementation time — goose over atlas: simpler, single binary, plain
SQL migrations that can wrap the frozen schema.sql verbatim without a DSL.)

---

## Web (Phase 4) — see entries added during that phase below

### [D10] Font + palette
(Recorded at implementation time if Phase 4 is reached.)

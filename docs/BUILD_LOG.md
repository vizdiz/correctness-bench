# Build log

Append-only, timestamped record of meaningful steps + verification output.
Times are UTC. Session started 2026-05-29.

---

## 2026-05-29T04:30Z — Phase 1: environment survey

Tooling present: go 1.26.2, node v24.15.0 / npm 11.12.1, gh 2.92.0 (authed as
`vizdiz`), brew 5.1.14, python3 3.12 (broken pip — pyexpat), git 2.54.0.

Missing: rust/cargo, docker, protoc, psql, wrk/wrk2, goose/atlas.

Actions:
- Installed Rust via rustup → `rustc 1.96.0`, `cargo 1.96.0`. (DECISIONS D1)
- `brew install protobuf` → `libprotoc 35.0`. (DECISIONS D4)
- Declined to install Docker/colima unsupervised. (DECISIONS D2)

## 2026-05-29T04:36Z — Phase 1: contract validators

All four kit validators run, ALL PASS:

```
$ protoc --proto_path=contracts --descriptor_set_out=/dev/null contracts/bench.proto
PROTO OK: bench.proto parses, descriptor generated

JSON fenced blocks in contracts/api.md: 9 found, 9 parse OK
YAML frontmatter in .claude/agents/*.md: 6 files, all valid (keys: name,description,tools,model)
contracts/schema.sql via libpg_query (pg-query-emscripten): parsed 17 statements, no errors
```

No fixes needed — contracts are clean (and frozen). SQL validated with the real
libpg_query parser compiled to wasm (DECISIONS D3), not a naive scan.

## 2026-05-29T04:36Z — Phase 1: monorepo scaffold

- Moved `build-kit/*` to repo root (CLAUDE.md, contracts/, gates/, .claude/).
  Kit README → `BUILDKIT_README.md`; wrote a proper project `README.md`.
- Created layout: engine/ (stub lib + sentinel), control/ (go mod +
  main.go stub), web/ (vite react-ts scaffold), mock/ (impl this session),
  cli/ (stub bin), mcp/ (TS stub), docs/, .gitignore (Rust+Go+Node),
  docker-compose.yml (postgres+mock+control+web; engine commented).
- `go mod init github.com/vizdiz/correctness-bench/control` → go.mod ok.
- `npm create vite@latest web -- --template react-ts` → scaffolded ok.

<!-- Subsequent phases append below this line. -->

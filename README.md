# Local Agent Memory (LAM) — v1.0

Local-first structured memory for coding agents on one machine: Codex, Claude
Code, OpenCode, and Hermes share one SQLite-backed memory per project with temporal
facts, a knowledge graph, a document vault, hybrid retrieval, admission
policies, and dreaming/consolidation. No daemon, no network calls in the core,
no cloud dependency. Source of truth is `LOCAL_AGENT_MEMORY_SDD.md`.

## Quick start

```sh
cargo build --release -p agent-memory
export AGENT_MEMORY_HOME=/tmp/lam-home
./target/release/agent-memory init
./target/release/agent-memory status
./target/release/agent-memory doctor
```

Or install and uninstall with the scripts (temp-HOME safe, data preserved by
default):

```sh
sh scripts/install.sh --prefix ~/.local
sh scripts/uninstall.sh --prefix ~/.local
```

The installer detects which agent hosts are present (`~/.codex`,
`~/.claude`, `~/.config/opencode`, `~/.hermes`, or their binaries on `PATH`)
and wires hooks plus the `agent-memory` MCP server into each one
automatically. Pass `--no-integrate` to skip that step, or
`--hosts codex,claude-code` to limit it. Idempotent: rerun any time; every
write is backed up and unrelated host settings are preserved.

```sh
agent-memory integration install auto     # detect and wire all present hosts
agent-memory integration status           # shows detected + installed state
agent-memory integration uninstall auto   # remove only what is installed
```

## Everyday use

```sh
agent-memory project bind --path . --name my-project
agent-memory memory add --project . --kind lesson --text "Timeouts can come from UDP media, not signaling"
agent-memory memory search --project . --query "timeout media"
agent-memory memory context --project . --task "fix the timeout" --token-budget 200
agent-memory maintain --project . --budget-ms 30000
agent-memory backup create
agent-memory export --project . --format jsonl --output backup.jsonl
agent-memory project policy --project . --capture on --sensitivity standard
```

## Workspace

- `crates/lam-core`: typed entities, errors, JSON schemas, clocks, fixtures
- `crates/lam-config`: config schema, defaults, validation, platform paths
- `crates/lam-storage`: SQLite WAL/FK/busy-timeout, migrations, schema checks
- `crates/lam-adapters`: host hook normalization and redaction
- `crates/lam-providers`: local-only extraction/embedding providers
- `crates/lam-vault`: allowlisted document scanner, chunking, sync
- `crates/lam-retrieval`: FTS/vector fusion, context budgets, injection guards
- `crates/lam-consolidate`: durable jobs, extraction/dedup/dream pipelines, admission
- `apps/agent-memory`: CLI (`init/status/doctor/backup/index/privacy/maintain/schedule/memory/vault/hook/mcp/...`)
- `scripts/`: `install.sh`, `uninstall.sh`, `release.sh`, `profile.sh`
- `fixtures/`: session, fact, and redaction fixture samples
- `docs/`: specs, ADRs, implementation status, runbook, testing evidence

## Quality gates

```sh
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --workspace
```

Full acceptance evidence: `docs/testing/e2e-matrix.md` (AT-001 to AT-013),
`docs/testing/benchmark.md`, `docs/testing/performance.md`,
`docs/testing/final-demo.md`. Parity statement:
`docs/testing/dewee-parity.md`. Release checklist and rule compliance:
`docs/implementation-status.md`.

License: MIT OR Apache-2.0. See `LICENSE-MIT` and `LICENSE`.

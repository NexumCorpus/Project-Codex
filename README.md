# Nexum Graph

**AI-native code coordination for multi-agent software engineering.**

Nexum Graph is a deterministic coordination layer that enables multiple AI coding agents to work on the same codebase without corrupting each other's changes. It parses code into a semantic graph, provides intent-based locking with CRDT-backed distributed convergence, validates changes against lock ownership, and maintains an async event log for rollback and replay.

Zero lines of Rust were written by a human. Claude wrote the types, tests, and prompts. OpenAI Codex filled in every function body.

## Architecture

Five-layer deterministic chassis:

```
 Layer 1 ─ Semantic Code Graph    tree-sitter CST → petgraph DiGraph
 Layer 2 ─ Intent Coordination    semantic locking, CRDT sync, conflict detection
 Layer 3 ─ Continuous Validation  pre-commit lock coverage checks
 Layer 4 ─ Immutable Event Log    async append-only log, rollback, replay
 Layer 5 ─ IDE Integration        LSP shim + HTTP coordination server
```

## Workspace

8 crates in a Cargo workspace:

| Crate | Purpose |
|---|---|
| `nex-core` | Authoritative types (`SemanticUnit`, `SemanticId`, `SemanticDiff`, `DepKind`) |
| `nex-parse` | Tree-sitter extraction, `SemanticExtractor` trait, TypeScript + Python + Rust extractors |
| `nex-graph` | `petgraph` semantic graph, diff algorithm |
| `nex-coord` | Conflict detection, coordination engine, intent lifecycle, Loro CRDT replica sync |
| `nex-validate` | Pre-commit validation (lock coverage, broken references, stale callers) |
| `nex-eventlog` | Async event log with pluggable backends (local-file default, JetStream opt-in) |
| `nex-lsp` | `tower-lsp` proxy with semantic diff, lock annotations, event streaming |
| `nex-cli` | CLI binary with 10 subcommands |

## Supported Languages

| Language | Extractor | Constructs |
|---|---|---|
| TypeScript / TSX | `nex-parse::typescript` | Functions, classes, methods, interfaces, enums, type aliases |
| Python | `nex-parse::python` | Functions, classes, methods, decorators, async variants |
| Rust | `nex-parse::rust` | Functions, structs, enums, traits, impl methods, inline modules |

## Installation

### Prerequisites

- Rust 1.85+ (edition 2024)
- Git (for repository operations)
- Python 3.10+ (optional, for developer tools)

### Build from source

```bash
git clone https://github.com/NexumCorpus/Nexum-Graph.git
cd Nexum-Graph
cargo build --release
```

The binary is at `target/release/nex` (or `target/release/nex.exe` on Windows).

### Verify

```bash
cargo test --workspace
```

All 186 tests should pass.

## Commands

### `nex diff` — Semantic diff between two git refs

```bash
nex diff v1 v2 --format text
nex diff main feature-branch --format json
```

Computes a semantic diff showing added, removed, and modified code units (functions, classes, methods) between two git refs. Understands signature vs body-only changes.

### `nex check` — Conflict detection between branches

```bash
nex check branch-a branch-b
nex check feature-1 feature-2 --format json
```

Three-way merge analysis that detects four conflict types:
- **Concurrent Modification** — same function modified on both branches
- **Signature Drift** — function signature changed, callers may break
- **Broken Reference** — dependency target deleted or moved
- **Stale Caller** — caller body unchanged but callee signature changed

### `nex lock` — Acquire a semantic lock

```bash
nex lock alice validate write
nex lock bob processRequest read
```

Requests a semantic lock on a named code unit. Lock kinds: `read`, `write`, `delete`. Multiple read locks are compatible; write and delete locks are exclusive. Lock state is persisted to `.nex/coordination.loro` (CRDT) with a `.nex/locks.json` compatibility snapshot.

### `nex unlock` — Release a semantic lock

```bash
nex unlock alice validate
```

### `nex locks` — List active locks

```bash
nex locks
nex locks --format json
```

### `nex validate` — Check lock coverage

```bash
nex validate alice --base HEAD~1
nex validate bob --base main --format json
```

Validates that all modifications in the working tree are covered by semantic locks held by the named agent. Reports unlocked modifications, unlocked deletions, broken references, and stale callers.

### `nex log` — Event history

```bash
nex log
nex log --intent-id <uuid> --format json
```

Shows the semantic event log. The default backend writes to `.nex/events.json`; set `NEX_EVENTLOG_BACKEND=jetstream` to publish events into a NATS JetStream stream instead.

### `nex rollback` — Semantic rollback

```bash
nex rollback <intent-id> alice
```

Generates compensating mutations that reverse a prior intent's changes. Detects conflicts if later events modified the same units.

### `nex replay` — Replay state to a point in time

```bash
nex replay --to <event-id>
```

Applies all mutations in order up to the specified event, producing the semantic state at that point.

### `nex serve` — Coordination server

```bash
nex serve --host 127.0.0.1 --port 4000
```

Starts an HTTP + WebSocket coordination server exposing:
- `POST /intent/declare` — declare intent with automatic lock acquisition
- `POST /intent/commit` — commit intent, append to event log, release locks
- `POST /intent/abort` — abort intent, release locks
- `GET /graph/query` — query the semantic graph
- `GET /locks` — list active locks
- `GET /events` — WebSocket stream of coordination events

## LSP Server

The `nex-lsp` binary provides IDE integration via the Language Server Protocol:

```bash
nex-lsp --repo-path . --base-ref HEAD~1
```

Custom LSP methods:
- `nex/semanticDiff` — file-scoped semantic diff
- `nex/activeLocks` — lock annotations as code lenses
- `nex/agentIntent` — intent declaration from IDE actions
- `nex/validationStatus` — real-time validation diagnostics
- `nex/eventStream` — semantic event notifications

The LSP also supports upstream proxy pass-through, forwarding standard LSP requests to an existing language server while injecting Nexum Graph overlays.

## Developer Tools

Python-based tools for spec-driven development workflows:

### `tools/spec_query.py` — Search implementation specs

```bash
python tools/spec_query.py locking --doc spec
python tools/spec_query.py "CRDT coordination" --mode phrase --stats
python tools/spec_query.py "semantic.*lock" --mode regex --max-results 5 --json
```

Extracts text from `.docx` spec documents and searches with configurable modes (`all`, `any`, `phrase`, `regex`). Caches extracted text with mtime-based invalidation.

### `tools/verify_slice.py` — Targeted workspace verification

```bash
python tools/verify_slice.py --crate nex-coord
python tools/verify_slice.py --changed
python tools/verify_slice.py --since origin/main --json
python tools/verify_slice.py --list-crates
```

Derives the workspace dependency DAG from live `cargo metadata`, infers impacted crates from changed files, expands transitive dependents, and runs `cargo test` + `cargo clippy` + `cargo fmt --check` for the affected crate set.

### `tools/workspace_doctor.py` — Workspace health check

```bash
python tools/workspace_doctor.py
python tools/workspace_doctor.py --legacy-scan --json
```

Checks toolchain availability, spec document presence, local Codex skill installation, dirty-tree impact analysis, and optional legacy-name scanning.

## Configuration

### Environment Variables

| Variable | Default | Description |
|---|---|---|
| `NEX_EVENTLOG_BACKEND` | `local-file` | Event log backend (`local-file` or `jetstream`) |
| `NEX_NATS_URL` | `nats://127.0.0.1:4222` | NATS server URL (JetStream backend) |
| `NEX_EVENTLOG_STREAM` | `nex_events` | JetStream stream name prefix |
| `NEX_EVENTLOG_SUBJECT_PREFIX` | `nex.events` | NATS subject prefix |
| `NEX_COORD_PEER_ID` | Auto-derived | CRDT peer ID override (default: BLAKE3 hash of hostname + repo path) |

### State Directory

Nexum Graph stores local state in `.nex/` at the repository root:

| File | Purpose |
|---|---|
| `coordination.loro` | CRDT document for distributed lock convergence |
| `locks.json` | Backward-compatible JSON lock snapshot |
| `events.json` | Local event log (when using `local-file` backend) |
| `cache/` | Tool caches (spec text extraction) |

## How It Works

1. **Parse**: Tree-sitter parses source files into concrete syntax trees. The `SemanticExtractor` trait maps CST nodes to `SemanticUnit` values with content-addressed IDs (BLAKE3), signature hashes, and normalized body hashes.

2. **Graph**: Units and their dependency edges (calls, imports, inheritance, implementations) form a `petgraph` directed graph. Diffing two graphs produces added/removed/modified classifications.

3. **Coordinate**: Agents declare intents targeting specific units. The coordination engine acquires semantic locks, checks for conflicts with existing lock holders, and manages intent lifecycles (declare -> commit/abort) with TTL-based expiry. CRDT-backed state enables distributed replica convergence via `export_crdt` / `merge_crdt`.

4. **Validate**: Before commit, the validation engine checks that every modification is covered by a write lock, every deletion by a delete lock, and flags broken references and stale callers.

5. **Log**: Committed intents produce immutable semantic events with structured mutations. The async event log supports compensating rollback and state replay, with pluggable backends for local files or NATS JetStream.

## Numbers

- **186** tests, all passing
- **5** architectural layers
- **10** CLI subcommands
- **8** workspace crates
- **3** language extractors (TypeScript, Python, Rust)
- **5** custom LSP methods
- **0** lines of Rust written by a human

## License

MIT

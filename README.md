# Nexum Graph

**AI-native code coordination for multi-agent software engineering.**

Nexum Graph is a deterministic coordination layer that enables multiple AI coding agents to work on the same codebase without corrupting each other's changes. It parses code into a semantic graph, provides intent-based locking, validates changes against lock ownership, and maintains an immutable event log for rollback and replay.

Zero lines of Rust were written by a human. Claude wrote the types, tests, and prompts. OpenAI Codex filled in every function body.

## Architecture

Five-layer deterministic chassis:

```
 Layer 1 ─ Semantic Code Graph    tree-sitter CST → petgraph DiGraph
 Layer 2 ─ Intent Coordination    semantic locking, conflict detection
 Layer 3 ─ Continuous Validation  pre-commit lock coverage checks
 Layer 4 ─ Immutable Event Log    append-only log, rollback, replay
 Layer 5 ─ IDE Integration        LSP shim + HTTP coordination server
```

## Workspace

8 crates in a Cargo workspace:

| Crate | Purpose |
|---|---|
| `nex-core` | Authoritative types (`SemanticUnit`, `SemanticId`, `SemanticDiff`, `DepKind`) |
| `nex-parse` | Tree-sitter extraction, `SemanticExtractor` trait, TypeScript + Python extractors |
| `nex-graph` | `petgraph` semantic graph, diff algorithm |
| `nex-coord` | Conflict detection, coordination engine, intent lifecycle |
| `nex-validate` | Pre-commit validation (lock coverage, broken references, stale callers) |
| `nex-eventlog` | Append-only event log, compensating rollback, state replay |
| `nex-lsp` | `tower-lsp` proxy with semantic diff, lock annotations, event streaming |
| `nex-cli` | CLI binary with 10 subcommands |

## Installation

### Prerequisites

- Rust 1.85+ (edition 2024)
- Git (for repository operations)

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

All 165 tests should pass.

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

Requests a semantic lock on a named code unit. Lock kinds: `read`, `write`, `delete`. Multiple read locks are compatible; write and delete locks are exclusive.

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

Shows the semantic event log (`.nex/events.json`). Each event records an agent's committed intent with mutations, parent event, and tags.

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
- `nex/validationStatus` — real-time validation diagnostics
- `nex/eventStream` — semantic event notifications

## How It Works

1. **Parse**: Tree-sitter parses source files into concrete syntax trees. The `SemanticExtractor` trait maps CST nodes to `SemanticUnit` values with content-addressed IDs (BLAKE3), signature hashes, and normalized body hashes.

2. **Graph**: Units and their dependency edges (calls, imports, inheritance) form a `petgraph` directed graph. Diffing two graphs produces added/removed/modified classifications.

3. **Coordinate**: Agents declare intents targeting specific units. The coordination engine acquires semantic locks, checks for conflicts with existing lock holders, and manages intent lifecycles (declare → commit/abort) with TTL-based expiry.

4. **Validate**: Before commit, the validation engine checks that every modification is covered by a write lock, every deletion by a delete lock, and flags broken references and stale callers.

5. **Log**: Committed intents produce immutable semantic events with structured mutations. The event log supports compensating rollback and state replay.

## Numbers

- **165** tests, all passing
- **5** architectural layers
- **10** CLI commands
- **8** workspace crates
- **0** lines of Rust written by a human

## License

MIT

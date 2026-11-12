# 0007: Code layout and lane ownership

- Status: Accepted
- Date: 2026-11-11
- Deciders: Maintainers

## Context

Phase 1 and Phase 2 code is built by several lanes working in parallel:
node reader, book and bars, bus with gateway and client, archive,
historical API, and infrastructure. They share one data model. Without a
fixed layout and clear ownership, parallel lanes collide on shared files,
and a change to a shared type silently breaks another lane.
[AGENTS.md](../../AGENTS.md) requires the code layout to be a decision
record.

## Decision

### Repository layout

| Path | Contents |
|---|---|
| `Cargo.toml`, `Cargo.lock` | One Cargo workspace for all Rust code |
| `rust-toolchain.toml` | Pinned toolchain |
| `deny.toml` | Dependency license, ban and advisory policy (`cargo deny`) |
| `crates/` | Libraries and services, one crate per lane |
| `tools/` | Generators and command-line tools |
| `fixtures/synthetic/` | Generated synthetic test data |
| `fixtures/admitted/` | Small samples from admitted sources, each directory with an `ADMISSION` file naming the `ADM-NNNN` record |
| `docs/specs/` | Data specifications (data model, source formats, manifests, archive layout) |
| `docs/protocol/` | Wire protocols (gateway) |
| `docs/operations/` | Runbooks |
| `deploy/` | Containers and infrastructure as code (infrastructure lane) |

### Crates and lanes

| Crate | Kind | Lane | Responsibility |
|---|---|---|---|
| `crates/mesh-types` | library | integrator | Shared data model, encodings, hashes, signatures, manifests |
| `crates/hl-node-reader` | library and binary | node reader | Hyperliquid node files to records, field allowlist, sequencing |
| `crates/book` | library | book and bars | L4 books, L2 and top of book, reconciliation, bars and other derived records |
| `crates/bus` | library | bus, gateway and client | NATS JetStream subjects, publish and replay |
| `crates/gateway` | library and binary | bus, gateway and client | Public websocket gateway |
| `tools/mesh-client` | library and binary | bus, gateway and client | Reference client |
| `crates/archive` | library and binary | archive | Filtered raw capture, partition writer, replay, manifests |
| `crates/history-api` | library and binary | historical API | HTTP range queries over partitions |
| `tools/hl-synth` | library and binary | integrator | Synthetic node-format fixture generator |
| `crates/swap-pollers` | library and binary | swap pollers | CFTC Part 43 and SEC SBSR public dissemination to `SwapPrint` records |
| `tools/hl-backfill` | library and binary | backfill | Hyperliquid public archive buckets to private raw, filtered raw and partitions |
| `deploy/` | not a crate | infrastructure | Containers, infrastructure as code |

### Ownership rules

1. A lane edits only its own crate (or `deploy/` for infrastructure) and
   its own documentation under `docs/operations/`.
2. Lanes do not edit `crates/mesh-types`, the root `Cargo.toml`,
   `deny.toml`, `docs/specs/`, `docs/protocol/`, CI, `scripts/` or another
   lane's crate. Changes to shared things go to the integrator as a
   request (issue or pull request against those files only), and land
   before the lane depends on them.
3. A lane adds dependencies to its own crate's `Cargo.toml`. Versions for
   expected needs are pre-declared in `[workspace.dependencies]`; use
   `name.workspace = true` where one exists. A dependency needed by a second
   crate moves into the workspace table through the integrator.
4. `Cargo.lock` is committed. When two lanes change it, the integrator
   regenerates it on merge.
5. Every dependency must pass `cargo deny check`: permissive licenses only,
   no duplicate-major bans violated, no known advisories.
6. Lanes build and test against `fixtures/synthetic/`. Code that ingests a
   real source merges only after its admission is `Admitted`.

### Code rules

- Rust edition 2024, toolchain pinned in `rust-toolchain.toml`.
- `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`
  and `cargo test --workspace` pass on every pull request.
- No `unwrap()` in library code paths outside tests; `unsafe` is forbidden.
- Each crate's `README.md` states its responsibility and status.

## Consequences

- Lanes can work in parallel with no shared-file conflicts except
  `Cargo.lock`, which the integrator resolves mechanically.
- A shared type change is a deliberate, reviewed step, so a lane cannot
  break another lane by editing `mesh-types`.
- The integrator is a bottleneck for shared changes. The data model is
  specified up front (`docs/specs/observation-v0.md`) to keep such changes
  rare.

## Alternatives considered

- One crate per service with shared types copied: drifts, and breaks
  determinism between live and archive paths.
- A polyglot layout (for example, a Python archive writer): two
  implementations of canonical encoding and hashing to keep identical.
- Letting lanes edit `mesh-types` freely: fastest locally, but the most
  likely source of cross-lane breakage.

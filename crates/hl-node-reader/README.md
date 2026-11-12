# hl-node-reader

Hyperliquid node output reader for the bleh.xyz mesh.

**Status:** implemented against the synthetic fixtures; not yet run
against a live node (see the assumptions to verify in
[docs/operations/node-reader.md](../../docs/operations/node-reader.md)).

## Responsibility

Tails the node's per-block output files (inotify via `notify`, with a
polling fallback), parses them into mesh-types records for every market in
the node's metadata, enforces the field allowlist (excluded fields are
dropped at parse time), assigns sequence numbers, merges two nodes with
integrity alarms, and hands one `BlockBatch` per instrument, kind and
block to the bus. Also produces filtered raw, the public reproducibility
root, and maps L4 snapshots from the node's info server.

| Module | Contents |
|---|---|
| `assumptions` | Every node-format fact still marked "to verify", one name each |
| `raw`, `fills`, `normalize`, `join`, `sequence` | Pure parsing: lines to records to batches |
| `filter` | Filtered raw (`filter_line`, `filter_raw_line`) |
| `snapshot` | L4 snapshots (`parse_l4_snapshot`, `filter_l4_snapshot`, `request_l4_snapshot`) |
| `markets` | Registry from node metadata |
| `files`, `replay`, `tail` | Reading hourly files once or live |
| `merge`, `pipeline`, `checkpoint` | Active-active merge, wiring, restart |
| `http`, `metrics` | Info server client, Prometheus metrics |

Binary: `hl-node-reader` (see the runbook for flags).

## Rules

- This crate is owned by one build lane. Other lanes do not edit it.
- Shared types come from `mesh-types`. Changes to `mesh-types` or the
  workspace `Cargo.toml` go through the integrator.
- See [ADR 0007](../../docs/decisions/0007-code-layout.md).

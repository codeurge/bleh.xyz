# hl-backfill

Backfill from Hyperliquid's public archive buckets (ADM-0003) into the
private raw tier, filtered raw and partitions, plus an order-level replay
prototype.

Status: built and tested against synthetic data only; no live AWS call
runs in tests. Owned by the backfill lane; see
`docs/decisions/0007-code-layout.md`.

| Module | Responsibility |
|---|---|
| `buckets` | Buckets, admitted datasets, the hard-coded denylist (`asset_ctxs`, `misc_events_by_block`), key parsing |
| `listing` | Listing admitted prefixes by day, height, coin and shard |
| `estimate` | Coverage and requester-pays cost from listings |
| `copy`, `checkpoint`, `retry` | Verbatim, resumable, concurrent copy with backoff |
| `store` | Object stores from the environment (credentials never logged) |
| `codec` | LZ4 frames in, zstd out |
| `filter`, `parse`, `partition` | Traits for the integrator, with local implementations |
| `clock` | Time to block for data without block numbers |
| `process` | Raw tier to filtered raw and partitions |
| `compare` | Backfill against live recording per hour |
| `replay` | Emulated matcher and L2 checkpoints |

Commands: `estimate`, `copy`, `process`, `compare`, `replay-prototype`.
Runbook: `docs/operations/backfill.md`. Bucket layouts:
`docs/specs/hyperliquid-archive-buckets.md`.

# history-api

Historical query API for the bleh.xyz mesh.

**Status:** implemented and tested against a synthetic archive. Runbook:
[docs/operations/history-api.md](../../docs/operations/history-api.md).

## Responsibility

A small, stateless service over the static, Hive-partitioned Parquet
archive ([archive-layout-v0](../../docs/specs/archive-layout-v0.md)):

- **Query engine:** DuckDB (bundled) over local paths, `s3://`/`r2://`
  object stores, or plain HTTPS through the daily manifests, with per-query
  timeouts, a memory cap, a concurrency cap, row limits and opaque cursors.
  Precomputed bars, footprints and volume profiles are read, never
  recomputed.
- **JSON API** (`/v0`): instruments, trades, book events, bars, footprints,
  volume profiles, book at time T, coverage, manifests, OpenAPI, limits;
  CORS, per-IP rate limits, request logging, Prometheus `/metrics`,
  `/health`.
- **Websocket replay** (`/v0/replay`) in the gateway-v0 message format at
  N× speed.
- **Compatibility layers:** Databento-shaped `timeseries.get_range` in DBN
  (Apache-2.0 `dbn` crate), TradingView UDF datafeed, CCXT-style REST shim,
  and an MCP server (official `rmcp` SDK; stdio and streamable HTTP).
- **Demo:** `history-api demo` writes a synthetic archive and serves it.

## Layout

| Module | Contents |
|---|---|
| `config`, `error`, `params` | Configuration and limits, error codes, time/range/cursor parsing |
| `engine` | DuckDB pool, timeouts, memory cap, locked configuration |
| `source`, `store`, `rows` | File resolution (walk, glob, manifests), typed queries, rows to `mesh_types` records |
| `book` | Order-level book reconstruction (checkpoint plus events) |
| `service` | Operations shared by every surface |
| `http::{v0, replay, databento, udf, ccxt, openapi}` | HTTP surfaces |
| `mcp` | MCP tools and transports |
| `metrics` | Prometheus text exposition |
| `synth` | Synthetic archive generator and Parquet writer (demo and tests) |

## Rules

- This crate is owned by one build lane. Other lanes do not edit it.
- Shared types come from `mesh-types`. Changes to `mesh-types` or the
  workspace `Cargo.toml` go through the integrator.
- See [ADR 0007](../../docs/decisions/0007-code-layout.md).

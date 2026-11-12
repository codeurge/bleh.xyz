# Plans

Each plan owns its goal, acceptance criteria and ordered tasks. Update task
status in the same pull request as the work.

| Plan | Status | Next action |
|---|---|---|
| [Phase 0: foundation](phase-0-foundation.md) | Active | Accept ADR 0007 (code layout); data-model and registry schemas |
| [Phase 1: live data](phase-1-live-data.md) | Active | Cost model, then two-provider hosting; synthetic fixtures, node reader, book and bar builders, block signing; Part 43 and SBSR pollers |
| [Phase 2: historical data](phase-2-historical-data.md) | Active | Copy Hyperliquid's public buckets now; manifest and anchoring spec; partition writer; `replica_cmds` replay |

Phases 3–6 get plans when Phases 1 and 2 near exit. See
[docs/roadmap.md](../../docs/roadmap.md).

## Build lanes (Phases 1–2)

| Lane | Plan |
|---|---|
| Node-reader: all markets, inotify | Phase 1 |
| Book, bars and analytics | Phase 1 |
| Bus, gateway, relay and client SDKs | Phase 1 (discovery in Phase 2) |
| Archive: filtered raw, partitions, manifests, per-block signing, anchoring, torrents and IPFS | Phase 2 (block signing in Phase 1) |
| Static history, small API and compatibility layers | Phase 2 |
| Infrastructure: active-active, multi-provider, CDN | Phase 1 (re-origination in Phase 2) |
| Parallel: Part 43 and SBSR pollers | Phase 1 |
| Parallel: backfill | Phase 2, starting now |

## Task statuses

`Ready`, `In progress`, `In review`, `Blocked`, `Done`.

## Task entry template

```md
N. **<task-slug>**
   - Status: Ready
   - Owner: <name or Unassigned>
   - Dependencies: <tasks, or None>
   - PR: <link, or Not opened>
   - Next action: <one concrete step>
```

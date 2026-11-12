# Plans

Each plan owns its goal, acceptance criteria and ordered tasks. Update task
status in the same pull request as the work.

| Plan | Status | Next action |
|---|---|---|
| [Phase 0: foundation](phase-0-foundation.md) | Active | Counsel review of ADM-0001–0003; decide ADR 0004; data-model and registry schemas |
| [Phase 1: live data](phase-1-live-data.md) | Active | Decide ADR 0006; choose hosting; synthetic fixtures, book and bar builders |
| [Phase 2: historical data](phase-2-historical-data.md) | Active | Manifest spec; partition writer; ADM-0003; raw archive starts with the Phase 1 node |

Phases 3–6 get plans when Phases 1 and 2 near exit. See
[docs/roadmap.md](../../docs/roadmap.md).

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

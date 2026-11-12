# Plans

Each plan owns its goal, acceptance criteria and ordered tasks. Update task
status in the same pull request as the work.

| Plan | Status | Next action |
|---|---|---|
| [Phase 0: foundation](phase-0-foundation.md) | Active | Counsel review of ADM-0001 and ADM-0002; decide ADR 0004 |

Phases 1–5 get their own plans when Phase 0 exits. See
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

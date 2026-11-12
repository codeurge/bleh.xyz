# Decision records

Each significant decision gets a numbered record, using
[0000-template.md](0000-template.md). Records are never deleted. A reversed
decision is marked `Superseded by NNNN`.

Statuses: `Proposed`, `Accepted`, `Rejected`, `Superseded by NNNN`.

| # | Decision | Status |
|---|---|---|
| [0001](0001-record-decisions.md) | Record decisions as ADRs | Accepted |
| [0002](0002-licensing.md) | Apache-2.0 for code, CC0 1.0 for data and methods | Accepted |
| [0003](0003-field-level-admission.md) | Admit sources per field, with recorded decisions | Accepted |
| [0004](0004-wire-format.md) | DBN-compatible format with provenance sidecar | Accepted (Phase 2 compatibility layer) |
| [0005](0005-working-name.md) | "bleh.xyz" is a working name only | Superseded by 0008 |
| [0006](0006-phase-1-2-service-architecture.md) | Service architecture for live and historical data | Accepted |
| [0007](0007-code-layout.md) | Code layout and lane ownership | Accepted |
| [0008](0008-project-name.md) | The project is named bleh.xyz | Accepted |
| [0009](0009-admission-policy-amendments.md) | Admission policy amendments: open buckets, venue-matched prices | Accepted |
| [0010](0010-budget-first-hosting.md) | Budget-first hosting: one rented node box plus a self-hosted archive server | Accepted |
| [0011](0011-v0-amendments-for-the-origin.md) | Data model and gateway amendments for the integrated origin | Accepted |
| [0012](0012-open-terms-route-for-public-bodies-and-market-operators.md) | Admission policy amendment: open-terms route for public bodies and regulated market operators; `exchange_referenced` flag | Accepted |
| [0013](0013-synthetic-contract-months.md) | Synthetic contract months without exchange data | Accepted |
| [0014](0014-user-supplied-data-stays-local.md) | User-supplied data stays on the user's machine (bring your own feed) | Accepted |
| [0015](0015-v0-amendments-for-reference-pollers.md) | Data model amendments for the reference pollers: `exchange_referenced` bit, `positioning` kind, reference-value periods, manifest notices | Accepted |


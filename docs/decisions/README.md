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
| [0008](0008-project-name.md) | The project is named bleh.xyz | Accepted |
| [0009](0009-admission-policy-amendments.md) | Admission policy amendments: open buckets, venue-matched prices | Accepted |

ADR 0007 (code layout) arrives with the code foundation.

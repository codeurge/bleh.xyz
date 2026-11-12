# 0004: DBN-compatible wire format with provenance sidecar

- Status: Proposed
- Date: 2026-11-11
- Deciders: Maintainers

## Context

Many market-data tools already read Databento's DBN binary format. If the
mesh emits DBN-compatible records, those consumers can switch with little
work. DBN's fixed record layouts may not carry the mesh's provenance fields
(admission ID, node, signature, flags).

## Decision (proposed)

- Emit market records (trades, book events, bars) as DBN-compatible records
  under the mesh's own dataset and publisher IDs.
- Carry provenance in a sidecar channel keyed by record, or in an extension
  record type.
- Also offer Arrow and JSON for simple clients.

## Open before acceptance

1. Confirm the DBN specification's and libraries' licenses allow this use.
2. Check whether an extension record type is possible without breaking
   existing DBN readers.
3. Make sure using the format implies no affiliation or endorsement.

## Alternatives considered

- A bespoke binary format: full control, but no existing readers.
- Arrow only: good for analytics, weak for low-latency streaming.

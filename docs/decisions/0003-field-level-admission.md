# 0003: Admit sources per field, with recorded decisions

- Status: Accepted
- Date: 2026-11-11
- Deciders: Maintainers

## Context

Some sources mix clean and encumbered fields. For example, on-chain venues
publish their own trades and orders, but some also publish oracle or mark
prices that may be derived from exchange data.

## Decision

- Admission is decided per source **and field set**, under
  [admission-policy.md](../admission-policy.md).
- Each decision is a record in [docs/admissions/](../admissions/), and every
  observation carries the ID of the admission that covers it.
- Ingest code may emit only fields covered by an `Admitted` record.

## Consequences

- Venues can be used for their clean fields without importing their oracle
  risk.
- Removing a source is mechanical: find observations by admission ID.
- Admission review is the main gate and needs two approvals.

## Alternatives considered

- Whole-source admission: it would either exclude useful venues or let
  oracle fields in.

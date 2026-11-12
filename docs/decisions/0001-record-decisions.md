# 0001: Record decisions as ADRs

- Status: Accepted
- Date: 2026-11-11
- Deciders: Maintainers

## Context

The project's credibility rests on clean lineage and reproducible methods.
Consumers, contributors and node operators need to see why each choice was
made.

## Decision

Decisions that affect the data model, wire format, admission policy, methods,
license or governance are recorded as numbered ADRs in `docs/decisions/`.

## Consequences

- A pull request making such a change without an ADR is incomplete.
- Records are append-only. Reversals supersede, they do not delete.

## Alternatives considered

- Decisions in issues only: they are easy to lose and hard to cite.

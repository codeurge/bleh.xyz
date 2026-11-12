# 0008: The project is named bleh.xyz

- Status: Accepted
- Date: 2026-11-11
- Deciders: Repository owner
- Supersedes: [0005](0005-working-name.md)

## Context

ADR 0005 treated "bleh.xyz" as a placeholder. The owner holds the bleh.xyz
domain, and the name fits the project: a shrug at the incumbents it makes
less necessary.

## Decision

The project's name is **bleh.xyz**, used everywhere: repository, crates,
packages, dataset and publisher IDs, key namespaces, domains and docs.

## Consequences

- Identifiers may use the name now. ADR 0005's caution about hard-coding it
  no longer applies.
- Public endpoints live under the bleh.xyz domain (for example
  `live.bleh.xyz`, `data.bleh.xyz`), with the discovery mechanisms in
  ADR 0006 as fallbacks if a domain is lost.

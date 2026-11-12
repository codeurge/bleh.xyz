# 0005: "bleh.xyz" is a working name only

- Status: Superseded by 0008
- Date: 2026-11-11
- Deciders: Maintainers

## Context

The repository was created under the folder name `bleh.xyz`. A public name
needs a trademark search and must not suggest affiliation with any exchange,
vendor or venue.

## Decision

Use "bleh.xyz" internally until a public name is chosen. The public name is a
Phase 0 task and gets its own ADR.

## Consequences

- Avoid hard-coding the name in identifiers that would be costly to change
  later (package names, dataset IDs, key namespaces) until the public name is
  decided.

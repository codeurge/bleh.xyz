# 0002: Apache-2.0 for code, CC0 1.0 for data and methods

- Status: Accepted
- Date: 2026-11-11
- Deciders: Maintainers

## Context

The goal is data that anyone may use without obligations. Code needs a
permissive license with a patent grant, because the project touches market
infrastructure where patents exist.

## Decision

- Code: Apache License 2.0 ([LICENSE](../../LICENSE)).
- Data, archive contents, registry entries and method documents: CC0 1.0
  Universal ([LICENSE-DATA](../../LICENSE-DATA)).

## Consequences

- Downstream users carry no attribution or share-alike obligations for data.
- Contributors need not assign copyright. DCO sign-off is enough (see
  [CONTRIBUTING.md](../../CONTRIBUTING.md)).
- CC0 does not override third-party rights. That is why the admission policy
  keeps encumbered data out in the first place.

## Alternatives considered

- MIT for code: no explicit patent grant.
- ODbL or CC-BY for data: their attribution and share-alike duties work
  against "freely available with no strings".

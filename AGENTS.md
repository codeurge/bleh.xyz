# AGENTS.md

Canonical instructions for every contributor to this repository, human or
agent. `CLAUDE.md` points here.

## What this project is

An open, freely redistributable market-data mesh. Its value depends on one
property: **clean lineage**. No exchange-licensed data, and nothing derived
from it, may ever enter the repository, the archive or the network. Read
[docs/admission-policy.md](docs/admission-policy.md) before touching sources.

## Hard rules

1. **No exchange data, ever.** Do not commit, paste, quote, fixture or test
   against data from an exchange feed, an exchange-data vendor, a broker
   screen or an oracle network. This includes small samples and "just for a
   test". Test fixtures must be synthetic or come from an admitted source,
   with the admission ID recorded next to the fixture.
2. **Sources need an admission record first.** Code that ingests a source may
   merge only after its admission record in [docs/admissions/](docs/admissions/)
   reaches `Admitted`, and only for the admitted fields.
3. **No private details.** Do not name or describe private consumers,
   companies or individuals beyond what they have agreed to make public.
   Requirements from consumers are written generically.
4. **Decisions are recorded.** Anything that changes the data model, the
   wire format, the admission policy, a method (composites, conversions,
   settlement), the license or governance needs a decision record in
   [docs/decisions/](docs/decisions/).
5. **Methods are versioned.** A change to a published method gets a new
   version. Old versions stay documented and reproducible.
6. **Clean-room code.** Source readers are written from documented schemas
   and specifications only. Never copy code from a repository that has no
   license, or a license incompatible with Apache-2.0 (for example
   `hyperliquid-dex/order_book_server`, which has no license file).

## Workflow

- **Repository owner:** the owner, and agents working directly for the owner,
  may commit and push straight to `main`. The checks still apply: run
  `scripts/check.sh` before pushing, and sign off every commit.
- **Everyone else:** changes go through a branch and a pull request. Do not
  push to `main`.
- Branch names: `<type>/<short-slug>`, where type is one of `feat`, `fix`,
  `docs`, `chore`, `adr`, `admission`, `method`.
- Commits follow [Conventional Commits](https://www.conventionalcommits.org/)
  and carry a DCO `Signed-off-by:` line (`git commit -s`). See
  [CONTRIBUTING.md](CONTRIBUTING.md).
- One concern per pull request. Admission records and the code using them go
  in separate pull requests, with the admission merged first.
- Track work in [.agents/plans/](.agents/plans/). Update the owning plan's
  task status in the same pull request as the work.
- Update [CHANGELOG.md](CHANGELOG.md) under `Unreleased` for any change a
  consumer or contributor would notice.

## Layout

| Path | Purpose |
|---|---|
| `crates/` | Rust workspace members (see ADR 0007) |
| `tools/` | Command-line tools, such as replay and verifiers (see ADR 0007) |
| `fixtures/synthetic/` | Synthetic test data; the only place data files may live without an admission (see ADR 0007) |
| `docs/` | Design documents |
| `docs/specs/` | Machine-readable schemas and specifications (see ADR 0007) |
| `docs/protocol/` | Gateway and archive protocol documentation (see ADR 0007) |
| `docs/operations/` | Runbooks (see ADR 0007) |
| `docs/decisions/` | Decision records (ADRs) |
| `docs/admissions/` | One record per source admission decision |
| `.agents/plans/` | Plans with tracked tasks |
| `scripts/` | Repository checks |
| `.github/` | CI and templates |

The code layout is decided in ADR 0007.

## Checks

Run before opening a pull request:

```bash
scripts/check.sh
```

CI runs the same checks: forbidden-content scan, data-file guard, relative
link check, DCO sign-off, and Rust `fmt`, `clippy` and tests.

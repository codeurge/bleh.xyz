# Contributing

Thank you for helping build an open alternative to licensed market data.
This project has stricter rules than most, because one contaminated input can
compromise everything downstream. Please read this file and
[AGENTS.md](AGENTS.md) before opening a pull request.

## Before you start

- Open an issue for anything larger than a small fix, so the approach can be
  agreed first.
- Proposing a new data source? Open an **admission request** issue. Do not
  write ingest code until the admission record is `Admitted`. See
  [docs/admission-policy.md](docs/admission-policy.md).

## Never contribute

- Data from exchange feeds, exchange-data vendors, broker platforms or oracle
  networks, in any amount, for any purpose, including tests.
- Your own fills or quotes from an exchange. Only off-exchange (bilateral or
  OTC) trades of your own may be contributed, through the contributor program
  once it exists.
- Text copied from exchange rulebooks, contract specifications or websites.
  Write registry entries in your own words from public facts.

## Developer Certificate of Origin

Every commit must be signed off, certifying the
[Developer Certificate of Origin 1.1](https://developercertificate.org/):

```bash
git commit -s
```

By signing off you also confirm that the contribution contains no data or
text covered by the "Never contribute" list above.

## Pull requests

Contributors other than the repository owner submit every change as a pull
request (see [AGENTS.md](AGENTS.md) § Workflow).

- Use a branch named `<type>/<short-slug>`.
- Follow [Conventional Commits](https://www.conventionalcommits.org/).
- Keep one concern per pull request.
- Run `scripts/check.sh` and make sure CI passes.
- Update the owning plan in `.agents/plans/` and `CHANGELOG.md`.
- Changes to the data model, wire format, methods, admission policy, license
  or governance need a decision record in `docs/decisions/`.

## Review

- Every pull request needs one approving review from a maintainer other than
  the author.
- Admission records and method changes need two approvals.

## Code of conduct

Participation is governed by [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md).

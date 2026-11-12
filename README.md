# bleh.xyz — open market-data mesh

> Working name. Status: design. No code yet.

An open-source, freely redistributable source of futures-grade market data
that never touches exchange-licensed feeds.

Exchange groups control their prices through data contracts. Anyone who
receives those prices, or data derived from them, inherits the contract. This
project takes a different route. It collects prices for the **same economic
exposure** from places where that exposure trades in the open, and combines
them with published, reproducible methods:

- on-chain order-book venues, read from our own nodes
- swap trades that US law requires to be published free of charge
- public-domain cash-market and rates data from US agencies

Arbitrage keeps these prices close to exchange prices. The link is economic,
not contractual.

## What it aims to provide

- Trade-by-trade tapes with native aggressor side
- Order-by-order books, for the venues that publish them
- Bars from 1 second to 1 day, and a permanent, tamper-evident archive
- Composite prices per exposure (for example "S&P 500 front", "WTI front", "gold")
- Dated-contract equivalents for perpetual markets, from an open cost-of-carry model
- An open daily reference close, recomputable by anyone
- Rate curves, cash-market prices and positioning data
- An open instrument registry: contract specs, rolls, session calendars

It does **not** aim to provide exchange prices, official settlements, or
execution-grade marks for positions held at an exchange. See
[docs/requirements.md](docs/requirements.md).

## Principles

1. **Clean lineage over coverage.** A source joins only if nothing upstream of
   it carries an exchange data contract. See
   [docs/admission-policy.md](docs/admission-policy.md).
2. **Provenance on every record.** Source, venue, license and lineage travel
   with the data. Node operators sign what they publish.
3. **Reproducible outputs.** Composites, conversions and settlements use
   published methods, and anyone can recompute them from the archive.
4. **Honest labels.** Proxies are called proxies. The basis to the reference
   market is measured and published, not hidden.
5. **No single point of control.** Independent nodes, several gateways, and a
   content-addressed archive.

## Documents

| Document | Contents |
|---|---|
| [docs/requirements.md](docs/requirements.md) | What consumers need, and the targets this project sets |
| [docs/sources.md](docs/sources.md) | The source register: tiers, terms, and what still needs checking |
| [docs/admission-policy.md](docs/admission-policy.md) | Rules a source must meet to join |
| [docs/architecture.md](docs/architecture.md) | Data model, nodes, composites, settlement, transport, archive |
| [docs/coverage.md](docs/coverage.md) | Expected coverage by asset class |
| [docs/roadmap.md](docs/roadmap.md) | Phases, exit criteria, open questions |

## Project records

| Record | Contents |
|---|---|
| [docs/decisions/](docs/decisions/) | Decision records (ADRs) |
| [docs/admissions/](docs/admissions/) | Source admission register; no source is admitted yet |
| [.agents/plans/](.agents/plans/) | Tracked plans; Phase 0 is active |
| [CHANGELOG.md](CHANGELOG.md) | Notable changes |

## Contributing

Read [CONTRIBUTING.md](CONTRIBUTING.md) and [AGENTS.md](AGENTS.md). Changes
go through pull requests, and commits need a DCO sign-off. Never contribute
exchange-licensed data in any form. Report suspected contamination privately,
as described in [SECURITY.md](SECURITY.md).

## License

Code is licensed under [Apache-2.0](LICENSE). Data, registry entries and
method documents are dedicated to the public domain under
[CC0 1.0](LICENSE-DATA). See [ADR 0002](docs/decisions/0002-licensing.md);
counsel confirms this before the first public release.

## Not legal advice

These documents record engineering research. Every source's terms need review
by counsel before the project publishes data from it.

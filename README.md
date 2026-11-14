<p align="center">
  <a href="https://bleh.xyz">
    <picture>
      <source media="(prefers-color-scheme: dark)" srcset="brand/logo-light.svg">
      <img src="brand/logo.svg" alt="bleh.xyz" width="320">
    </picture>
  </a>
</p>

<h3 align="center">An open market-data mesh with clean lineage</h3>

<p align="center">
  Futures-grade market data that never touches exchange-licensed feeds.<br>
  Signed, reproducible and free to redistribute.
</p>

<p align="center">
  <a href="https://github.com/codeurge/bleh.xyz/actions/workflows/checks.yml"><img src="https://github.com/codeurge/bleh.xyz/actions/workflows/checks.yml/badge.svg?branch=main" alt="checks"></a>
  <a href="https://github.com/codeurge/bleh.xyz/actions/workflows/deploy.yml"><img src="https://github.com/codeurge/bleh.xyz/actions/workflows/deploy.yml/badge.svg?branch=main" alt="deploy"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/code-Apache--2.0-blue" alt="Code: Apache-2.0"></a>
  <a href="LICENSE-DATA"><img src="https://img.shields.io/badge/data-CC0--1.0-lightgrey" alt="Data: CC0 1.0"></a>
  <a href="rust-toolchain.toml"><img src="https://img.shields.io/badge/rust-1.99-orange?logo=rust" alt="Rust 1.99"></a>
  <a href="CONTRIBUTING.md"><img src="https://img.shields.io/badge/DCO-sign--off%20required-green" alt="DCO sign-off required"></a>
</p>

<p align="center">
  <a href="https://bleh.xyz"><b>Website</b></a> ·
  <a href="https://data.bleh.xyz/reference-normalized/_manifests/latest.json"><b>Open archive</b></a> ·
  <a href="docs/architecture.md"><b>Architecture</b></a> ·
  <a href="docs/roadmap.md"><b>Roadmap</b></a> ·
  <a href="docs/admission-policy.md"><b>Admission policy</b></a> ·
  <a href="CONTRIBUTING.md"><b>Contributing</b></a>
</p>

---

> **Status:** Phases 0–2 active; live and historical services in development.
> See the [roadmap](docs/roadmap.md) and the [changelog](CHANGELOG.md).

## Why bleh.xyz

Exchange groups control their prices through data contracts. Anyone who
receives those prices, or data derived from them, inherits the contract. This
project takes a different route. It collects prices for the **same economic
exposure** from places where that exposure trades in the open, and combines
them with published, reproducible methods:

- on-chain order-book venues, read from our own nodes
- swap trades that US law requires to be published free of charge
- public-domain cash-market and rates data from US agencies

Arbitrage keeps these prices close to exchange prices. The link is economic,
not contractual. **No exchange-licensed data, and nothing derived from it,
ever enters the repository, the archive or the network.**

## What it aims to provide

- **Tapes:** trade-by-trade tapes with native aggressor side, for every
  market on each venue, signed block by block
- **Books:** order-by-order books, for the venues that publish them
- **Order flow:** open footprint, delta, volume profile and heatmaps
- **Bars and archive:** bars from 1 second to 1 day, and a versioned archive
  that anyone can download over HTTPS, BitTorrent or IPFS and verify against
  signed manifests anchored on-chain
- **Composites:** composite prices per exposure (for example "US large-cap
  index front", "WTI front", "gold")
- **Dated-contract equivalents** for perpetual markets, from an open
  cost-of-carry model
- **Open reference close:** an open daily reference close, recomputable by
  anyone, and an on-chain oracle with no exchange data in its lineage
- **Rates and positioning:** rate curves, cash-market prices and positioning
  data
- **Open instrument registry:** contract specs, rolls, session calendars

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
5. **No single point of control.** Active-active ingest, signed blocks that
   any relay can carry, and a content-addressed archive from day one;
   independent operators as the mesh federates. No project token.

## Quick start

You need `rustup` (it installs the pinned toolchain from
[rust-toolchain.toml](rust-toolchain.toml) automatically) and a C++ compiler
with `make`, for the bundled DuckDB and zstd builds. A full build takes a
while the first time.

```bash
git clone https://github.com/codeurge/bleh.xyz.git
cd bleh.xyz
cargo build --workspace
cargo test --workspace
```

Run a local gateway on a synthetic feed and stream from it with the
reference client:

```bash
# Terminal 1: demo gateway on 127.0.0.1:8080, signed with an in-memory key
cargo run -p gateway -- demo --ephemeral-key

# Terminal 2: stream trades for 60 seconds
cargo run -p mesh-client -- --url ws://127.0.0.1:8080/v0/ws --sub '*|trade' --duration 60
```

Verify a day of the public archive, as a mirror would
([archive runbook](docs/operations/archive.md),
[manifest spec](docs/specs/manifest-v0.md)):

```bash
cargo run -p origin --bin archive -- verify \
  --url https://data.bleh.xyz/reference-normalized --archive reference-normalized \
  --date 2026-10-28 --trusted 0x<signer address>
```

Build the service container images from a commit (reproducible, tagged by
git SHA; see [deploy/](deploy/)):

```bash
deploy/docker/build.sh                 # every target, tagged with HEAD's SHA
deploy/docker/build.sh origin archive  # selected targets
```

## Open data

The `reference-normalized` archive is public at
[`data.bleh.xyz/reference-normalized`](https://data.bleh.xyz/reference-normalized/_manifests/latest.json):
signed daily manifests ([ADR 0017](docs/decisions/0017-records-archives.md))
with OpenTimestamps proofs, BitTorrent v2 magnets in
[`_manifests/magnets.txt`](https://data.bleh.xyz/reference-normalized/_manifests/magnets.txt),
IPFS CIDs and static Iceberg metadata. Data is CC0; some admitted sources
carry a notice that must stay with their values. Read the
[terms of use](docs/terms.md) before you redistribute.

| Specification | Contents |
|---|---|
| [observation-v0](docs/specs/observation-v0.md) | Record model ([JSON Schema](docs/specs/observation-v0.schema.json), [Arrow schema](docs/specs/observation-v0.arrow-schema.json)) |
| [gateway-v0](docs/protocol/gateway-v0.md) | Live WebSocket protocol: subscriptions, signed blocks, inclusion proofs |
| [manifest-v0](docs/specs/manifest-v0.md) | Signed daily archive manifests and verification |
| [archive-layout-v0](docs/specs/archive-layout-v0.md) | Archive file layout |
| [registry-v0](docs/specs/registry-v0.md) | Instrument registry ([JSON Schema](docs/specs/registry-v0.schema.json)) |
| [docs/specs/](docs/specs/) | Every spec and method |

## Repository layout

| Path | Contents |
|---|---|
| [crates/](crates/) | Rust workspace: `origin` (node reader, book builder, block signer, archive CLI), `gateway`, `bus`, `book`, `archive`, `history-api`, `swap-pollers`, `reference-pollers`, `mesh-types`, `contracts` |
| [tools/](tools/) | `mesh-client` (reference client), `hl-synth` (synthetic feed), `hl-backfill` |
| [fixtures/synthetic/](fixtures/synthetic/) | Synthetic test data; the only place data files may live without an admission |
| [deploy/](deploy/) | Container images, compose profiles, Terraform, systemd units, observability |
| [docs/](docs/) | Design documents, specs, protocol, runbooks, decisions, admissions |
| [brand/](brand/) | Logo, palette, site and design system |

The layout is decided in [ADR 0007](docs/decisions/0007-code-layout.md).

## Documents

| Document | Contents |
|---|---|
| [docs/requirements.md](docs/requirements.md) | What consumers need, and the targets this project sets |
| [docs/sources.md](docs/sources.md) | The source register: tiers, terms, and what still needs checking |
| [docs/admission-policy.md](docs/admission-policy.md) | Rules a source must meet to join |
| [docs/terms.md](docs/terms.md) | Terms of use and disclaimer for bleh.xyz and its data |
| [docs/architecture.md](docs/architecture.md) | Data model, nodes, composites, settlement, transport, archive |
| [docs/coverage.md](docs/coverage.md) | Expected coverage by asset class |
| [docs/roadmap.md](docs/roadmap.md) | Phases, exit criteria, open questions |
| [docs/operations/](docs/operations/) | Runbooks for every service, deploys, keys and incidents |

## Project records

| Record | Contents |
|---|---|
| [docs/decisions/](docs/decisions/) | Decision records (ADRs) |
| [docs/admissions/](docs/admissions/) | Source admission register; Hyperliquid node and archive fields are admitted |
| [.agents/plans/](.agents/plans/) | Tracked plans; Phases 0–2 are active |
| [CHANGELOG.md](CHANGELOG.md) | Notable changes |

## Contributing

Contributions are welcome. Read [CONTRIBUTING.md](CONTRIBUTING.md) and
[AGENTS.md](AGENTS.md) first.

- Changes go through pull requests (the repository owner may push
  directly). Run `scripts/check.sh` before you open one; CI runs the same
  checks.
- Commits follow [Conventional Commits](https://www.conventionalcommits.org/)
  and need a DCO sign-off (`git commit -s`).
- A new source needs an [admission record](docs/admissions/) before any code
  that ingests it can merge.
- **Never contribute exchange-licensed data in any form**, not even a small
  sample for a test.

Everyone taking part follows the [code of conduct](CODE_OF_CONDUCT.md).

## Security

Report vulnerabilities and suspected data contamination privately, as
described in [SECURITY.md](SECURITY.md). Do not open a public issue.

## License

Code is licensed under [Apache-2.0](LICENSE). Data, registry entries and
method documents are dedicated to the public domain under
[CC0 1.0](LICENSE-DATA). See [ADR 0002](docs/decisions/0002-licensing.md).
Records from some admitted sources carry a notice that must stay with the
values; see the [terms of use](docs/terms.md).

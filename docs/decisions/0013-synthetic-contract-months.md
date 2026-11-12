# 0013: Synthetic contract months without exchange data

- Status: Accepted
- Date: 2026-11-12
- Deciders: Repository owner (accepted 2026-11-12); proposed by the synthetic
  contract-month lane

## Context

Consumers of futures data want a value for each contract month of an
exposure (equity index, crude oil, natural gas, gold, silver, short-term
rates). Exchange futures prices are licensed data and can never enter the
mesh ([admission policy](../admission-policy.md)). The mesh already holds,
or will hold once admitted, the ingredients of a fair value: independent
venue prices for the underlying exposures, public rates (government
yields, the overnight reference rate) and publicly disseminated swaps,
whose commodity calendar-month prints are forward prices. The contract
calendars themselves are published rulebook rules, not market data.

The [open reference prices plan](../../.agents/plans/open-reference-prices.md)
(task 2) asks for a versioned method and an engine that turn those
ingredients into a fair-value series per contract month, under neutral
names and with liquidity and quality flags. This changes the data model
(two new kinds) and introduces a method, so AGENTS.md rules 4 and 5 apply.

## Decision

1. **Method `synthetic-months@1`**, specified in
   [method-synthetic-months-v1.md](../specs/method-synthetic-months-v1.md)
   and published under CC0 like every method. Any change that alters an
   output digit is a new version; v1 stays documented and reproducible.
   Families in v1: equity index (cost of carry with a dividend yield),
   crude oil and natural gas (swap-print forward anchors, else financing
   only, flagged), gold and silver (cost of carry with a lease rate), and
   one- and three-month overnight-rate months (expected average or
   compounded rate from fixings and the curve). Treasury note and bond
   months are specified as v2.
2. **New kinds** in observation v0 (amendment, before the first public
   release, as in [ADR 0011](0011-v0-amendments-for-the-origin.md)):
   - `reference_value` (not derived, § 2.12): a published public
     reference value (rate, par or zero yield, price, index level) with
     tenor or maturity, conventions, `as_of_date` and revision, on
     instrument `<source>:ref:<series>`. Future reference pollers emit it.
   - `synthetic_month` (derived, § 2.13): exposure, contract month in the
     neutral notation `YYYY-MM`, rank after the roll rule (0 for a month
     past its roll date), last trading day and expiry instant, year
     fraction, fair value, spot used, financing rate, carry used and its
     source, anchors used, implied rate (short-rate months), the input
     timestamps, quality flags, every input admission, and an inherited
     liquidity grade and label. Instrument `synthetic:month:<exposure>:<month>`
     on the pseudo-venue `synthetic`.
   Both are flat, schema-driven kinds in Arrow (like the analytics
   kinds). Canonical encodings of existing kinds are unchanged; the fast
   encoder stays byte-identical.
3. **Neutral output names.** Exposure IDs and names describe the exposure
   (`us-large-cap-index`, `wti-crude`, `gold`); the registry may state the
   economic reference as a fact ("tracks the exposure of the ... futures
   months"). No exchange product name or index brand is an identifier or
   output name.
4. **Quality flags and grades.** Every record carries quality flags
   (stale spot or rates, single-source or dispersed spot, tape sources,
   curve extrapolation, missing fixings, estimated or unobserved carry,
   anchor interpolation, extrapolation and staleness, rolling, expiring)
   and a liquidity grade inherited from its spot sources (the second-best
   source grade, so no single source decides; `benchmark` only for A and
   B). Months with no traded input are `tape`.
5. **What is claimed.** A synthetic month is a model value computed from
   independent inputs. It is not an exchange price or settlement and does
   not claim to match one. The spec lists every approximation (§ 15).
6. **Determinism.** Arithmetic uses binary64 basic operations and the
   crate's own `exp` and `ln`, not the platform math library, and fixed
   rounding rules, so every output is reproducible bit for bit from the
   archived inputs. Records carry an inputs hash and count.
7. **Code.** `crates/synthetic` (library and `synthetic` binary) owns the
   engine, calendars, curve, composite and registry; the repository ships
   only synthetic registry entries and fixtures. Wiring the engine into
   the origin, so synthetic months are signed and published like other
   derived kinds, is a later step behind a configuration flag.

## Consequences

- Consumers get a per-month fair-value series with explicit lineage,
  flags and grade, reproducible from published inputs, with no exchange
  data anywhere on the path.
- The values are only as good as their inputs: until the reference
  sources are admitted and polled (plan tasks 1 and 4), the engine runs
  on synthetic fixtures only, and carry is an estimate for most
  exposures.
- Venue markets used as spot proxies may track the front futures month
  rather than the cash underlying; the registry must say which, and its
  roll rule should match the venue market's.
- Readers of the archive see two new kinds and partitions under the
  pseudo-venue `synthetic` and the `<source>:ref:` instruments.
- Changing a formula, a calendar rule or a rounding rule now requires a
  new method version and a decision record.

## Alternatives considered

- **Volume-weighted roll.** Futures volume is exchange data; the roll is
  a fixed number of business days instead.
- **Publishing only a continuous front series.** Loses the term
  structure; each month is published and the front is a rank.
- **Platform `f64::exp` and `ln`.** Their last bit can differ between
  systems, which would break bit-for-bit reproducibility.
- **One record per exposure with every month nested.** Breaks per-month
  subscription and partitioning; months are instruments.
- **Using exchange settlement calendars as data files.** The rules are
  short and published; computing them in code avoids any data file and
  makes the edge cases testable.

# 0023: Reference values carry quantities: `stock`, `flow`, `capacity` and `ratio` measures with a `unit`

- Status: Accepted
- Date: 2026-11-15
- Deciders: Repository owner (proposed by the reference-pollers lane, which
  owned `crates/mesh-types` for this change)
- Amends: `observation-v0` § 2.12 and § 9 (as
  [ADR 0015](0015-v0-amendments-for-reference-pollers.md) did)

## Context

`reference_value` (observation-v0 § 2.12) holds rates, yields, prices and
index levels. Several admitted sources also publish quantities, and the
pollers read them and drop them, because no record kind holds a quantity:

- **EIA (ADM-0021).** Every admitted volume family: weekly petroleum
  stocks (Cushing included), refinery inputs, capacity and utilisation,
  production, imports, exports and product supplied; weekly natural gas
  storage; monthly crude oil production and natural gas supply and
  disposition. They are allowlisted, then counted as `unsupported_volume`.
- **New York Fed (ADM-0007).** `volumeInBillions`, the volume underlying
  SOFR, BGCR, TGCR, EFFR and OBFR, admitted and dropped.
- **USDA dairy (ADM-0011).** NDPSR sales volumes and the moisture content
  of barrel cheese, admitted and dropped.
- **USDA LMR (ADM-0010) and Market News feeder cattle (ADM-0012).** Head
  counts, admitted "for every purchase type" and "of every type", and not
  read.

Stocks and storage give carry and fair-value context for energy months
(ADM-0021, "Accuracy value"); volumes weight and qualify the rates and
prices already published.

A quantity needs what a price does not: a unit (thousand barrels, billion
cubic feet, head), and the difference between a level at a point in time
(a stock, which is never summed over time) and an amount over a period (a
flow, which is). Sources state units as free text ("Thousand Barrels per
Day", "Billion Cubic Feet"), so the text is not a stable column to group
or filter on.

## Decision

### 1. Four new measures in `reference_value`

`reference_value` stays the one kind for published values. `measure` gains
four values beside `rate`, `par_yield`, `zero_yield`, `price` and
`index_level`:

| Measure | Meaning | Examples |
|---|---|---|
| `stock` | A level at the end of the period that `as_of_date` stands for | Crude oil stocks at Cushing; working gas in storage |
| `flow` | An amount over the period, or its average rate when the unit is per day | Crude oil production; imports; NDPSR sales; head slaughtered; overnight funding volume |
| `capacity` | The most a facility can process or hold | Operable crude distillation capacity |
| `ratio` | A part of a whole, in percent | Refinery utilisation; cheese moisture content |

These four are the **quantity measures**. Day count, compounding and
currency stay null on them (the existing rules: rate conventions on rates
only, currency on prices only). `tenor` and `maturity_date` keep their
meaning and are allowed (an amount offered at an auction, for example,
has a maturity).

Specific measures follow the spec's style (`par_yield` and `zero_yield`
rather than `yield` with a flag). Stock against flow is what a consumer
must know to aggregate correctly. A ratio is not a stock or a flow, and
naming it `utilization` would not fit moisture content.

### 2. A `unit` field, from a closed vocabulary

`reference_value` gains `unit`, a string from this list:

| `unit` | Means | Base unit | Factor |
|---|---|---|---|
| `barrels` | Barrels of 42 US gallons | barrels | 1 |
| `thousand_barrels` | Thousands of barrels | barrels | 10^3 |
| `thousand_barrels_per_day` | Thousands of barrels per day | barrels per day | 10^3 |
| `million_cubic_feet` | Millions of cubic feet (MMcf) | cubic feet | 10^6 |
| `billion_cubic_feet` | Billions of cubic feet (Bcf) | cubic feet | 10^9 |
| `pounds` | Pounds | pounds | 1 |
| `metric_tons` | Metric tons | metric tons | 1 |
| `head` | Animals | head | 1 |
| `usd` | US dollars | US dollars | 1 |
| `million_usd` | Millions of US dollars | US dollars | 10^6 |
| `billion_usd` | Billions of US dollars | US dollars | 10^9 |
| `percent` | Percent | percent | 1 |

- **Required exactly on the quantity measures, absent otherwise.** Prices
  keep `currency` (and their unit of quantity stays in the registry
  entry's `unit` text, as today). Money amounts use the `usd` units, not
  `currency`, so a quantity's meaning is in one field.
- **Consistency.** `ratio` takes `percent` and nothing else, and
  `percent` is only for `ratio`. `stock` never takes a per-day unit.
- **Omitted, not `null`, when absent**, like `period_start_ms` (ADR
  0015). The canonical encoding, content hashes and signatures of every
  existing record are unchanged.
- **Adding a unit** is an additive change to the data model. It needs a
  decision record (this one may be amended while the first release is
  pending) and the spec's table, the JSON Schema and the enum change
  together.
- **Registry.** The record carries the unit, so every row is
  self-describing. The series catalogue (registry-v0 § 8) names the unit
  of each quantity family in its `unit` text as well, so a reader of the
  registry alone knows it. The record wins if they ever differ.

### 3. Scale and decimals: the published number, unchanged

`value` keeps its type (a fixed-point decimal whose scale is the number
of digits written, observation-v0 § 1) and is the number the source
published, in the unit the source published it in. A poller's reader maps
the source's unit text onto the vocabulary and never rescales: EIA's
"421234" thousand barrels is `value` `"421234"`, `unit`
`thousand_barrels`, not `421234000` barrels. This is how prices are
carried today (decimal text as published, no conversion), and it keeps
values comparable with the source. Consumers who want base units multiply
by the factor in the table, which is exact in decimal.

A reader that meets a unit text it does not know, or a unit that does not
fit the series' measure, drops the value and raises an unknown-field
alert (`units:<series>`), as it does today for a price that is not in
dollars.

### 4. Arrow and Parquet: one appended nullable column

The `reference_value` Arrow schema (§ 9) gains `unit` (`utf8`, nullable),
**after** `period_end_ms`. Appending keeps every existing column's
position and Parquet field ID; the new column takes the next ID.

- Files written before this change have no `unit` column. The archive's
  decoder reads a nullable column that is absent from a file as null, so
  old partitions still decode to the same records and the same content
  hash.
- The Iceberg metadata is rebuilt from scratch with the current schema
  (archive-layout § 3.3). Readers resolve by field ID, and a field ID
  absent from a data file reads as null, which is Iceberg's rule for
  added columns.
- DuckDB reads old and new files together with
  `read_parquet(..., union_by_name = true)` or through `iceberg_scan`;
  `measure` and `unit` are plain strings, so a query can filter
  `WHERE measure = 'stock' AND unit = 'thousand_barrels'` with no lookup.
  Prices have `unit` null and `currency` set.

### 5. Web snapshots

`reference/<source>.json` series (web-snapshot-v0) gain `unit` from the
latest record when it has one. Adding a field is compatible under that
spec's versioning, so the path stays `web/v0/`. Quantity series are told
apart by `measure` and `unit`; the series key (instrument, measure,
tenor, maturity) is unchanged.

### 6. Versioning

`observation-v0` is a draft, so this is an amendment of v0, as in ADRs 0011
and 0015, and the version tag does not change. After the first release, the
same change would be a minor version under § 10 (new enum values and an
optional field), since the canonical encoding of existing records is
untouched.

No method changes. `synthetic-months@1` does not read quantities, and a
method that uses them (for example storage-adjusted carry) would be a new
method version.

## Consequences

- The pollers can emit the volumes the admissions already allow. Which
  fields each source emits is in reference-sources.md and the runbook;
  every exclusion in the admissions still applies (EIA's allowlist and
  excluded families, vendor-credited series, LMR purchase-type rules for
  prices).
- New records appear in `reference-normalized`; no published record or
  partition changes. Old partitions keep their bytes and hashes.
- Readers of the previous v0 draft reject the new measures and `unit`
  (unknown values are rejected), so they fail loudly rather than misread a
  quantity as a price.
- The site's reference panel shows `value` with `currency`; for quantity
  series it should show `unit` instead. The site is a separate lane.

## Alternatives considered

- **A new `quantity` record kind.** It would duplicate everything
  `reference_value` already has (series instrument, `as_of_date`,
  revisions, periods, the revision store, the partition and snapshot
  paths) to add one field. Rejected.
- **One `quantity` measure.** Simpler, but it loses stock against flow,
  which decides whether values may be summed over time, and it would mix
  percentages with amounts.
- **The source's unit text in the record.** Free text varies by source and
  over time ("Thousand Barrels per Day", "Mbbl/d") and cannot be grouped
  on. Swap prints keep the disseminated text because Part 43 units are
  open-ended; reference sources publish a small, known set.
- **Converting to base units.** It changes the published number and
  makes values harder to check against the source, for no gain: the
  factor is a power of ten and exact.
- **The unit only in the registry.** Every Parquet row would need a join
  to the catalogue to be read, and pattern entries (`eia:ref:<series>`)
  cover series with different units.

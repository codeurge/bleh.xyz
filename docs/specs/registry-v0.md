# Instrument registry, v0

- Status: Draft, implemented by `crates/mesh-types` (`registry` module)
- Version tag: `registry-v0`
- JSON Schema: [registry-v0.schema.json](registry-v0.schema.json)
- Example: [fixtures/synthetic/registry/registry-v0.json](../../fixtures/synthetic/registry/registry-v0.json)
- Decision: [ADR 0018](../decisions/0018-registry-v0.md)

The registry says what the mesh's markets and series are: which venue
markets exist and how to read them, which economic exposure each one gives
under a neutral name, when those exposures trade and roll, and how liquid
they are. It is the file every service takes as `--registry`
(`hl-node-reader`, `origin`, `gateway`, `history-api`; see
[history-api.md](../operations/history-api.md)) and the document reviewed
in registry pull requests ([architecture.md](../architecture.md) § 10).

## 1. Principles

- **Facts, written fresh.** Every entry states a fact in the registry's own
  words and parameters. Nothing is copied from an exchange document or
  filled from exchange-licensed data
  ([admission-policy.md](../admission-policy.md)).
- **Neutral names.** Output names (exposure `id` and `name`) say what the
  data is: "US large-cap index front", "WTI crude front". Exchange product
  names, contract tickers and index brand names are never output names.
  An exchange contract or a named index may appear only in an exposure's
  `tracks` text, as the economic reference stated as a fact ("Tracks the
  front quarterly US large-cap equity index futures month").
- **Venue symbols are mappings, not identities.** An instrument's
  `venue_symbol` is the venue's own key for the market, kept so readers can
  match venue data. It is never shown as an output name, even when it
  happens to resemble an exchange ticker.
- **Exchange data stays out.** A field that could only be filled from
  exchange data is not in the schema (§ 9). Calendars and roll rules come
  from public rules (holiday law, published contract terms restated as
  parameters) or are left for the operator to fill.
- **Every market carries a liquidity grade**; an ungraded market is a
  `tape` (§ 7).

## 2. Document

One JSON object. Conventions follow
[observation-v0](observation-v0.md) § 1: unknown fields are rejected,
enumerations are lowercase snake_case, decimals are strings.

| Field | Type | Required | Meaning |
|---|---|---|---|
| `registry` | `"registry-v0"` | no | Version tag. Written whenever a section other than `instruments` is present |
| `instruments` | array of instrument (§ 3) | yes | Venue markets, sorted by `id` |
| `exposures` | array of exposure (§ 4) | no | Economic exposures and their venue mappings |
| `calendars` | array of calendar (§ 5) | no | Session calendars |
| `rolls` | array of roll schedule (§ 6) | no | Contract months, last trading days, roll rules |
| `series` | array of series entry (§ 8) | no | Reference series families |
| `liquidity` | array of liquidity entry (§ 7) | no | Latest liquidity grades |

A document with only `instruments` is the registry as it was before this
spec (`{"instruments": [...]}`) and stays valid; the implementation writes
it in that form. Empty sections are omitted.

IDs of exposures, calendars and roll schedules (entry IDs) match
`[a-z0-9][a-z0-9_-]{0,63}`. They never contain `:`, so an entry ID cannot be
confused with an instrument ID. Calendar, roll and exposure IDs are unique
within their section.

The JSON Schema checks shapes. The implementation also checks what a schema
cannot: that every `calendar` and `roll` reference resolves, that a
liquidity subject is a registered instrument or a listed exposure, that
`from` is not after `to`, and that multipliers are positive.

## 3. Instruments and venue mapping

An instrument is one venue market and everything a reader needs to
normalize it (`mesh_types::InstrumentSpec`, unchanged by this version). The
pair (`venue`, `venue_symbol`) is the **venue mapping**: it maps the
venue's key to the mesh ID. Both the ID and the pair are unique.

| Field | Meaning |
|---|---|
| `id` | Mesh instrument ID ([observation-v0](observation-v0.md) § 4) |
| `venue` | Venue ID |
| `class` | `perp`, `spot`, `outcome` or `swap` |
| `venue_symbol` | The market's key in the venue's own data (Hyperliquid: `coin`) |
| `namespace` | Deployer namespace of a builder-deployed market (HIP-3), if any |
| `price_scale`, `size_scale` | Fractional digits of prices and sizes, 0 to 18 |
| `flags` | Flags applied to every record of the market ([observation-v0](observation-v0.md) § 2.9) |
| `admission` | Admission record covering the market's fields |

A change to a registered market's scales changes how its decimals are
read. It needs an operator decision and a registry pull request, never an
automatic update ([node-reader.md](../operations/node-reader.md)).

### 3.1 Automatic registration from node metadata

Hyperliquid markets are registered automatically: there is no hard-coded
market list. `hl-node-reader` reads the info endpoint at start-up, when an
admitted coin it does not know appears, and every
`--registry-refresh-secs`, and writes the merged registry to
`--registry-out` ([hyperliquid-node-format.md](hyperliquid-node-format.md)
§ 8; `hl_node_reader::markets`). The mapping:

| Metadata | Instrument field |
|---|---|
| `meta` → `universe[].name` (core perpetual, e.g. `BTC`) | `venue_symbol`; `id` = `hyperliquid:perp:<name>`; `class` = `perp` |
| `perpDexs[].name`, then `meta` with `dex` → `universe[].name` | `venue_symbol` = `<dex>:<name>` (bare names get the prefix); `namespace` = `<dex>`; `class` = `perp` |
| `outcomeMeta` → `outcomes[].outcome` = `n` | Two instruments, `venue_symbol` `#<10n>` and `#<10n+1>`; `class` = `outcome` |
| `universe[].szDecimals` | `size_scale` |
| (derived) | `price_scale` = `6 - szDecimals` for perpetuals; `8 - szDecimals` for outcome markets, the spot rule ([ADR 0021](../decisions/0021-outcome-market-price-scale.md), assumption to verify) |
| `outcomes[].szDecimals`, if given | `size_scale` (default 0) |
| `universe[].isDelisted` | Ignored: delisted markets stay registered because history refers to them |
| (policy) | `flags` = `[oracle_anchored_venue]` for deployer (HIP-3) markets, `[]` otherwise; per-coin overrides allowed |
| (policy) | `admission` = `ADM-0001` |

Spot markets are outside ADM-0001 and are not registered. Metadata
responses shape the registry only; no metadata value becomes record
content.

Automatic registration fills `instruments` only. It never creates an
exposure, a name, a mapping to an exposure or a grade: those are reviewed
facts, added by registry pull request (§ 4) or by the grade publisher
(§ 7). A metadata spec that differs from the registered one (for example a
changed `szDecimals`) is reported as a conflict and the registered spec is
kept.

## 4. Exposures

An exposure is the economic thing a consumer wants, under a neutral name,
with the venue markets that give it.

| Field | Required | Meaning |
|---|---|---|
| `id` | yes | Neutral entry ID (`us-large-cap-index-front`) |
| `name` | yes | Neutral display name ("US large-cap index front") |
| `tracks` | yes | The economic reference, stated as a fact. The only place an exchange contract or named index may be named |
| `class` | yes | `crypto`, `equity_index`, `equity`, `energy`, `metal`, `agriculture`, `livestock`, `short_rate`, `rates`, `fx` or `other` |
| `form` | yes | `spot` (cash or spot level, no expiry), `front` (the front contract month on a roll schedule) or `months` (each listed month) |
| `quote` | no | Quote currency or unit of account (`USD`) |
| `unit` | no | Unit of the price ("index points", "US dollars per barrel") |
| `calendar` | no | Session calendar (§ 5) |
| `roll` | for `front` and `months` | Roll schedule (§ 6) |
| `venue_mappings` | no | Venue markets giving the exposure (§ 4.1) |
| `note` | no | Provenance of the facts, caveats |

Naming rules for `id` and `name`:

- Describe the asset and the curve point in plain words: region, size or
  grade, asset, then `front` or nothing for spot ("US large-cap index",
  "WTI crude front", "Gold front", "Bitcoin / US dollar").
- Commodity grades and delivery locations that are generic market terms
  may be used (WTI is a crude oil grade). Exchange product names, contract
  codes, tickers, index brand names and other trademarks may not.
- Do not reuse a venue symbol as a name.

Reviewers check these rules; they are not machine-checkable.

### 4.1 Venue mappings

| Field | Required | Meaning |
|---|---|---|
| `instrument` | yes | Mesh instrument ID of a venue market |
| `relation` | yes | `tracks_spot` (follows the cash or spot level), `tracks_front_month` (follows the front contract month) or `proxy` (related but a different reference, such as another grade or region; never used as a price reference for the exposure) |
| `multiplier` | no | Exposure units per unit of the market's price, positive decimal (default 1) |
| `from`, `to` | no | Dates (`YYYY-MM-DD`, inclusive) the mapping holds; `to` absent while current |

The instrument need not be in this document's `instruments`: markets of
other venues are registered by their own readers. Mappings are facts about
what a venue market is designed to follow (from the venue's or deployer's
public description). They are never inferred from exchange prices.

## 5. Session calendars

| Field | Required | Meaning |
|---|---|---|
| `id` | yes | Entry ID (`us_trading`, `continuous`) |
| `time_zone` | yes | IANA time zone of the session times (`America/New_York`, `UTC`) |
| `weekdays` | yes | Days of the week that are open before holidays, Monday first, no repeats |
| `holidays` | yes | Public holiday rule set: `none`, `us_trading` or `us_government_securities` |
| `sessions` | no | Sessions on an open day: `name`, `open` and `close` (`HH:MM`; `close` may be `24:00`; a close at or before the open falls on the next day). Empty: whole days only |
| `closures` | no | Extra closed dates (unscheduled closures such as a national day of mourning) |
| `openings` | no | Holidays that are open in a given year |
| `early_closes` | no | `date` and `close` of days that close early |
| `note` | no | Free text |

The holiday rule sets are the ones defined, computed and tested in
[method-synthetic-months-v1](method-synthetic-months-v1.md) § 5.1: public
holiday law and the Federal Reserve's observance rule, not data. A new
rule set needs a new minor version of this spec and its rules written out
in a method spec.

**Operator-filled fields.** `closures`, `openings` and `early_closes`
change from year to year with announcements. The registry carries them
only as operator entries from public announcements (government
proclamations, a public body's published schedule), never from an
exchange-data product. A session time is included only when it is a
widely published public fact (for example the regular US cash equity
session); otherwise `sessions` is left empty for the operator.

## 6. Roll schedules

A roll schedule says which contract months exist, when each stops trading
and when the front month changes. It uses the vocabulary of
[method-synthetic-months-v1](method-synthetic-months-v1.md) §§ 5.2 and 6,
so the same rule means the same dates everywhere.

| Field | Required | Meaning |
|---|---|---|
| `id` | yes | Entry ID (`quarterly-third-friday`) |
| `calendar` | yes | Business-day calendar (§ 5) |
| `cycle` | yes | Contract months, 1 to 12, ascending |
| `expiry` | yes | Last-trading-day rule: `nth_weekday {n, weekday, month_offset, business_days_before}`, `before_day {day, month_offset, business_days_before}` or `nth_last_business_day {n, month_offset}` (method spec § 5.2) |
| `settlement_time` | no | Wall time on the last trading day in the calendar's time zone, `HH:MM` |
| `roll` | yes | `{business_days_before, reference}` with `reference` `last_trade` or `month_start` (method spec § 6) |
| `note` | no | Free text |

The rules are published contract terms restated as parameters: a fact
such as "the third Friday of the contract month" is written fresh, never
copied. **Rolls are fixed-days only.** A roll on volume or open interest
would need exchange data and is not available (§ 9).

## 7. Liquidity grades

Liquidity grades are derived records (`liquidity_grade`,
[observation-v0](observation-v0.md) § 2.10, method `liquidity-grade@1` in
[book.md](../operations/book.md) § 6), published continuously. The
registry carries the latest grade per market or exposure so a consumer can
tell a benchmark from a tape without reading the stream.

| Field | Meaning |
|---|---|
| `subject` | Instrument ID (contains `:`) or exposure ID |
| `grade` | `A` to `D` |
| `label` | `benchmark` or `tape` |
| `as_of_ms` | Time of the grade (the record's `as_of_ms`) |
| `method` | Method that produced it (`liquidity-grade@1` for a venue market; the composite's method for an exposure, such as `synthetic-months@1` § 10) |

At most one entry per subject. **A subject without an entry is ungraded
and is a `tape`.** The label rule belongs to the method (the default
`benchmark_min` is `B`); the registry copies the record's label and does
not recompute it. Removing an instrument removes its entry.

## 8. Reference series

The reference pollers' catalogue (`reference-pollers catalog`,
`crates/reference-pollers/src/catalog.rs`) is the `series` section: one
entry per series family the pollers emit.

| Field | Meaning |
|---|---|
| `instrument` | Instrument ID, or a pattern with `<...>` placeholders (`frb-h15:ref:<SERIES_NAME>`) |
| `kind` | Record kind (`reference_value`, `positioning`) |
| `source` | Source name |
| `admission` | Admission record |
| `description`, `unit` | What the series is and the unit of its values |
| `flags` | Flags every record carries (for example `exchange_referenced`) |
| `notice` | Notice that travels with the series ([ADR 0012](../decisions/0012-open-terms-route-for-public-bodies-and-market-operators.md), decision 5) |
| `methods` | Method periods where the admission asks for them: `from`, `to` (`null` while current), `method` |

The pair (`instrument`, `kind`) is unique. Wrap the catalogue as
`{"registry": "registry-v0", "instruments": [], "series": [...]}` to get a
registry document.

## 9. Left out: fields that would need exchange data

These are not in the schema, on purpose. A pull request that adds them
needs an admission for the data and a decision record.

| Field | Why it is left out |
|---|---|
| Exchange contract codes, tickers, product names as identifiers | Trademarks and exchange naming; output names are neutral (§ 4). The economic reference goes in `tracks` as prose |
| Volume- or open-interest-based roll dates | Need exchange volume and open interest |
| Exchange settlement prices, daily limits, margins, tick values from exchange feeds | Exchange data |
| Contract sizes and tick sizes of exchange contracts | Not needed by any method; restating them invites copying exchange specification tables |
| Exchange holiday and early-close schedules as published data products | Licensed; holidays are computed from public rules (§ 5) |
| Index constituents and weights | Index-provider data |

## 10. Implementation

- `mesh_types::Registry` reads and writes the document. Instruments are
  indexed by ID and venue symbol; the other sections are
  `mesh_types::registry::Sections`, validated on load and by
  `Registry::set_sections`.
- Tests validate the JSON the code writes against the schema, with
  synthetic data and the catalogue only:
  `crates/mesh-types/tests/registry_schema.rs` (the example registry, the
  synthetic Hyperliquid registry `fixtures/synthetic/hl/markets.json`,
  registries built from `InstrumentSpec::hyperliquid`, and matching
  rejections between schema and code),
  `crates/hl-node-reader/tests/registry_schema.rs` (automatic registration
  from synthetic metadata) and
  `crates/reference-pollers/tests/registry_schema.rs` (the catalogue).
- The synthetic-months method keeps its own configuration file
  (`synthetic::registry`, method spec § 4) for v1. Its calendar, expiry
  and roll vocabulary is the same as §§ 5–6 here.

## 11. Versioning

`registry-v0` is a draft and may change until the first public release.
After that, adding an optional field, a section, an enumeration value or a
holiday rule set is a minor version; anything else is a new major version
with a decision record. Readers reject unknown fields, so a reader sees a
newer document as an error rather than guessing.

# Method: synthetic contract months, v1

- Status: Draft, implemented by `crates/synthetic`
- Method ID: `synthetic-months@1`
- Output kind: `synthetic_month` ([observation-v0](observation-v0.md) § 2.13)
- Decision: [ADR 0013](../decisions/0013-synthetic-contract-months.md)
- License: this method is published under CC0 1.0, like every method of
  the project ([ADR 0002](../decisions/0002-licensing.md)).

This method produces a fair-value series for each contract month of an
exposure (an equity index, crude oil, natural gas, gold, silver, a
short-term rate) **without exchange data**. Its inputs are records already
in the mesh or admitted public data; its rules are published rulebook
facts. Anyone can recompute every output from the archived inputs and this
document.

A change to anything in this document that changes an output digit is a
new version (`synthetic-months@2`), with this version kept documented and
reproducible ([AGENTS.md](../../AGENTS.md) rule 5).

## 1. What is and is not claimed

- A `synthetic_month` is a **model value**: what the independent inputs
  imply a contract month should be worth under the formulas below. It is
  not an exchange price, a settlement price or a quote, and it is not
  derived from any.
- It does not claim to match exchange settlement. Differences come from
  the spot proxy (venue markets, not the cash index or the futures), the
  carry inputs (estimates where not observed), the curve (public rates,
  not the market's financing rates), and the approximations listed in
  § 15.
- Every output carries its inputs' hash and count, its quality flags
  (§ 9), and a liquidity grade and label (§ 10). A `tape` label means the
  value must not be used as a reference price.
- Names are neutral (`us-large-cap-index`, `wti-crude`); the registry
  states the economic reference as a fact ("tracks the exposure of the
  ... futures months"), per the admission policy's naming rules.

## 2. Inputs

| Input | Records | Role |
|---|---|---|
| Spot | `trade`, `book_snapshot`, `queue_positions`, `liquidity_grade` of configured venue markets | Spot composite (§ 8) and grade (§ 10) |
| Overnight rate | `reference_value`, tenor `ON` | Front curve node and short-rate fixings (§§ 7, 12) |
| Term rates | `reference_value` (rates, par and zero yields) | Curve points (§ 7) |
| Overnight-index swaps | `swap_print`, asset class `interest_rate` | Curve points (§ 7) |
| Commodity swaps | `swap_print`, asset class `commodity` | Forward anchors (§ 11.2) |
| Carry series | `reference_value` | Observed carry (§ 11.1) |

Sources that feed `reference_value` and `swap_print` must be admitted for
those fields.

### 2.1 Rates from the reference pollers (informative)

`crates/reference-pollers` emits the admitted rate series as
`reference_value` records the engine reads unchanged. The preset curve
(`CurveConfig::reference_pollers()`, `synthetic --curve
reference-pollers`) maps them as follows; it changes no formula of this
method.

| Curve input | Instrument | Conventions | Admission |
|---|---|---|---|
| Overnight node and fixings | `nyfed:ref:SOFR` | rate, `ON`, simple act/360 | ADM-0007 |
| Set 1 (short end) | `us-treasury:ref:bill_rates_coupon_equivalent` | rate, tenor `{n}W` and the bill's `maturity_date`; simple act/365F (52 weeks: semi-annual) | ADM-0006 |
| Set 2 (beyond the longest bill) | `us-treasury:ref:par_yield_curve` | par yield, semi-annual, tenors `1M` to `30Y` (`1.5 Month` as `6W`) | ADM-0006 |

- A revised value (higher `revision` for the same date and tenor or
  maturity) replaces the earlier one, as § 7 already reads records.
- Backfilled records carry an estimated publication time (the
  pollers' documented release schedule), so a backfilled valuation sees a
  value only after it would have been published.
- Alternatives: the Board's H.15 constant maturities
  (`frb-h15:ref:RIFLGFC<u><n>_N.B`, par yields, same lineage as the
  Treasury curve) can replace set 2, and EFFR (`nyfed:ref:EFFR`) can be the
  overnight series of an exposure on that rate.

## 3. Arithmetic and rounding

- All arithmetic is IEEE 754 binary64 with the basic operations (`+`,
  `-`, `*`, `/`), which are correctly rounded on every conforming
  platform, and no fused multiply-add.
- `exp` and `ln` are the deterministic functions of
  `synthetic::detmath`: `exp(x)` reduces `x = k ln2 + r` with `ln2` split
  in two parts (high part `0x3FE62E42FEE00000`), evaluates the Taylor
  series of `e^r` to 24 terms in Horner form and scales by `2^k`; `ln(x)`
  splits `x = m 2^e` with `m` in `[sqrt(1/2), sqrt(2))` and sums 30 terms
  of `2 atanh((m - 1)/(m + 1))`. Platform math libraries are not used.
- Fixed-point inputs become binary64 as `mantissa / 10^scale`.
- Outputs are rounded **half away from zero**: `fair_value` and `spot` to
  the exposure's `price_scale`; `financing_rate`, `carry_rate` and
  `implied_rate` (percent) to 6 places. `year_fraction` is computed in
  integers: `round_half_up((expiry_ms - as_of_ms) * 10^8 / 31 536 000 000)`
  at 8 places.
- Times: act/365F years, `τ = (expiry_ms - as_of_ms) / 31 536 000 000`.

## 4. Registry

A JSON file (`synthetic::registry`) lists the curve inputs and the
exposures. The repository ships synthetic example entries only
([fixtures/synthetic/synthetic-months/registry.json](../../fixtures/synthetic/synthetic-months/registry.json));
operators add real venue market IDs. Per exposure:

| Field | Meaning |
|---|---|
| `id`, `name`, `tracks` | Neutral ID and name; the economic reference as a fact |
| `family` | `equity_index`, `energy`, `metal`, `short_rate` |
| `calendar` | `us_trading` or `us_government_securities` (§ 5.1) |
| `cycle`, `months_listed` | Contract months (month numbers) and how many non-rolled months to value |
| `expiry` | Last-trading-day rule (§ 5.2) |
| `settlement_time` | New York wall time on the last trading day (03:00 or later) |
| `roll` | Roll rule (§ 6) |
| `price_scale` | Fractional digits of `fair_value` and `spot` |
| `spot` | `tracks` (`cash` or `front_month`), sources (instrument, multiplier), `max_age_secs` (default 120), `hard_max_age_secs` (default 86 400), `max_dispersion_bps` (default 100) |
| `carry` | `series` (observed) and/or `estimate` (percent, continuous) with a provenance `note` |
| `anchors` | Swap-print anchors: `venue`, `underlier`, `window_hours`, `max_age_hours`, `multiplier` |
| `short_rate` | `compounded_quarter` or `average_month` |

The curve section names the overnight series, the point sets in priority
order and `max_age_business_days` (default 3).

## 5. Calendars

### 5.1 Business days

Computed in code (`synthetic::calendar`); public holiday rules, not data.

- **`us_trading`** (exchange trading days; equity index, energy and metal
  months): weekends; New Year's Day; Martin Luther King Jr. Day (third
  Monday of January); Washington's Birthday (third Monday of February);
  Good Friday (Easter Sunday minus two days, Gregorian computus); Memorial
  Day (last Monday of May); Juneteenth (19 June, from 2022); Independence
  Day (4 July); Labor Day (first Monday of September); Thanksgiving
  (fourth Thursday of November); Christmas (25 December). A fixed-date
  holiday on a Sunday is observed on Monday; on a Saturday, on Friday,
  except New Year's Day, which is then not observed.
- **`us_government_securities`** (overnight-rate fixing days; short-rate
  months and the `ON` tenor): the same weekday holidays plus Columbus Day
  (second Monday of October) and Veterans Day (11 November). A fixed-date
  holiday on a Sunday is observed on Monday; on a Saturday it is not
  moved (the Federal Reserve rule). Good Friday is a closure.
- **Exceptions** are configured per calendar: `closures` (unscheduled
  closures such as a national day of mourning) and `openings` (scheduled
  holidays that are open in a given year, such as a Good Friday on which
  the fixing is published).

Encoded edge cases, each tested: Juneteenth on a Friday (2026-06-19) and
on a Saturday (2027, observed 2027-06-18); New Year's Day on a Saturday
(2021-12-31 stays a trading day) and on a Sunday (2023-01-02 closed);
Independence Day on a Saturday (2026-07-03 closed for trading, open for
fixings) and on a Sunday (2027-07-05); Veterans Day on a Sunday (2029-11-12).

### 5.2 Last trading day

Published rulebook rules, parameterised (`ExpiryRule`):

| Rule | Definition |
|---|---|
| `nth_weekday {n, weekday, month_offset, business_days_before}` | The `n`-th `weekday` of the month `month_offset` after the contract month, then `business_days_before` business days earlier; with 0, a non-business day moves to the preceding business day |
| `before_day {day, month_offset, business_days_before}` | Calendar day `day` of the month `month_offset` after the contract month, moved to the preceding business day if it is not one, then `business_days_before` business days earlier |
| `nth_last_business_day {n, month_offset}` | The `n`-th last business day of the month `month_offset` after the contract month |

Rules per family in the example registry (published rulebook facts):

| Family | Cycle | Last trading day | Settlement time (New York) |
|---|---|---|---|
| US large-cap equity index | Mar, Jun, Sep, Dec | Third Friday of the month (preceding business day if a holiday): `nth_weekday {3, fri, 0, 0}` | 09:30 |
| WTI crude oil | Every month | 3 business days before the 25th of the prior month (from the preceding business day if the 25th is not one): `before_day {25, -1, 3}` | 14:30 |
| US natural gas | Every month | Third last business day of the prior month: `nth_last_business_day {3, -1}` | 14:30 |
| Gold | Feb, Apr, Jun, Aug, Oct, Dec | Third last business day of the month: `nth_last_business_day {3, 0}` | 13:30 |
| Silver | Mar, May, Jul, Sep, Dec | Third last business day of the month | 13:25 |
| Three-month overnight rate (SOFR) | Mar, Jun, Sep, Dec | Business day before the third Wednesday of the month three months on: `nth_weekday {3, wed, 3, 1}` | 17:00 |
| One-month overnight rate (SOFR) | Every month | Last business day of the month: `nth_last_business_day {1, 0}` | 17:00 |

`expiry_ms` is the last trading day at the settlement time in New York
local time: daylight time (UTC-4) from 02:00 on the second Sunday of
March to 02:00 on the first Sunday of November, standard time (UTC-5)
otherwise (rule in force since 2007).

### 5.3 Listed months

At valuation time `as_of_ms` with New York date `today`, the engine walks
the cycle from three months before `today`'s month, drops months whose
`expiry_ms < as_of_ms`, and keeps every remaining month past its roll date
(rank 0) plus the first `months_listed` months not past it (ranks 1, 2,
...). A month is valued up to and including its `expiry_ms`.

## 6. Roll rule

Futures volume is exchange data, so a volume-based roll is not available.
The front month changes on a **fixed number of business days** before a
reference day (`RollRule`):

- `reference = last_trade`: `business_days_before` business days before
  the last trading day (cash-settled and financially settled months);
- `reference = month_start`: before the first calendar day of the
  contract month, moved to the preceding business day if needed
  (physically delivered months, ahead of the delivery period).

The roll date never falls after the last trading day. From 00:00 New York
time on the roll date, the month has rank 0 and the flag `rolling`, and the
next month is the front (rank 1). It keeps being valued until expiry, so
each month's own series is continuous across the roll, and the front
series moves at the roll by exactly the calendar spread at that instant.

Example registry: equity index 8 business days before the last trading
day; crude oil and natural gas 3; gold and silver 2 business days before
the contract month starts; short-rate months 0 (rank 0 on the last trading
day).

## 7. Rate curve

The curve is a set of nodes `(t, z)`: horizon `t` in act/365F years and a
continuously compounded zero rate `z`. It is a **sticky-tenor** curve: a
point quoted for a tenor gives the rate for that horizon from the
valuation time, without rolling down; stale rates are flagged (§ 9), not
corrected.

### 7.1 Single-period quotes

For a quote over `n` calendar days (from the value's `as_of_date` to its
`maturity_date`, or to the end of its tenor: `nD` days, `nW` weeks, `nM`
months and `nY` years by calendar arithmetic clamped to month ends, `ON`
to the next `us_government_securities` business day; `TN` is not used),
with `t = n / 365`:

| Convention | Growth `G` | `z` |
|---|---|---|
| `simple`, day count `d` | `1 + r · α(n)` | `ln(G) / t` |
| `discount`, day count `d` | `1 / (1 - r · α(n))` | `ln(G) / t` |
| `annual` zero | | `ln(1 + r)` |
| `semi_annual` zero | | `2 ln(1 + r / 2)` |
| `continuous` | | `r` |

`α(n)` is `n / 360` for `act_360` and `thirty_360`, `n / 365` for
`act_365_fixed` and `act_act` (exact for the single periods read here).
Rates are percent in the records and fractions in the formulas.

### 7.2 Par yields

Par yields with `f` coupons a year (`annual` 1, `semi_annual` 2) are
bootstrapped per series: at coupon dates `k / f`, the par yield is linear
in `t` between points and flat before the first;
`D_k = (1 - (c_k / f) Σ_{j<k} D_j) / (1 + c_k / f)`. A point's zero rate
is `-ln D(t) / t`, with `ln D` linear between coupon dates; a point at or
below one coupon period is a single-period zero yield, `f ln(1 + c / f)`.

Overnight-index swap prints: the median fixed rate per tenor over the
set's trailing window (actions `new` and `correct`, event type `trade` or
absent, cancelled prints removed). Tenors up to 366 days are a single
simple act/360 period; longer tenors are annual par rates with the coupon
scaled by 365/360 (act/360 accrual), bootstrapped as above.

### 7.3 Building the curve

- The overnight point (newest `ON` value of the overnight series) is
  always a node.
- Point sets are taken in priority order. Each set contributes only
  points beyond the longest horizon of the sets before it, so sets never
  interleave (no zigzag between an overnight-index curve and a government
  curve). For a `series` set, every tenor of the newest `as_of_date` (not
  after `today`) of each series is used.
- Between nodes, `z·t` is linear in `t` (piecewise flat forwards). Before
  the first node `z` is flat; after the last node `z` is flat and the
  value is flagged `curve_extrapolated`.
- `financing_rate` is `z(τ)`. The growth between horizons is
  `exp(z(t2) t2 - z(t1) t1)`.
- Curve inputs (for the inputs hash): the overnight record and every
  record of each point set that contributes at least one node.

## 8. Spot composite

Per exposure, from its configured sources:

1. Each source's observation is the newer of its newest trade price and
   its newest top-of-book mid (`(best bid + best ask) / 2` from a book
   snapshot or queue positions), at or before `as_of_ms`; a mid wins a
   tie. Prices are multiplied by the source's `multiplier`.
2. **Fresh** sources (age at most `max_age_secs`) are eligible. If two or
   more eligible sources are labelled `benchmark` by their newest
   `liquidity_grade`, only those are used. Otherwise every eligible
   source is used, and if a `tape` or ungraded source is among them the
   value is flagged `spot_tape`.
3. With no fresh source, the single freshest source no older than
   `hard_max_age_secs` is used, flagged `spot_stale`. With none, the
   exposure is not valued (a notice is logged).
4. The composite is the **median** (mean of the two middle values for an
   even count). With three or more sources, sources further than
   `max_dispersion_bps` from the median are dropped and the median is
   recomputed (flag `spot_dispersed`); with two that differ by more than
   that, both are kept and the value is flagged.
5. One source used: flag `spot_single_source`.

**Removal tolerance.** With three or more sources, removing any one moves
the median by at most the spread of the remaining sources, never by an
outlier's error, as the admission policy requires of composites.

**Spot horizon.** A `cash` spot is a node at `t = 0`. A `front_month`
spot (a venue market that tracks the front futures month) is a node at
the front month's horizon `τ_front`; its own front month is valued at the
spot. The registry's roll rule should match the tracker's roll; a
mismatch shows as a step in non-front months at the tracker's roll.

Spot records used (each used source's observation and its grade record)
are inputs. `prov.flags` carries `oracle_anchored_venue` if any of them
does.

## 9. Quality flags and staleness

| Flag | Raised when |
|---|---|
| `spot_stale` | No spot source was fresh (§ 8 step 3) |
| `spot_single_source` | One spot source was used |
| `spot_dispersed` | A source was dropped as an outlier, or two sources disagree beyond tolerance |
| `spot_tape` | A `tape` or ungraded source was used |
| `rates_stale` | The newest curve date is more than `max_age_business_days` `us_government_securities` business days before `today` |
| `curve_extrapolated` | The horizon (expiry; for short-rate months, the end of the reference period) is past the last curve node |
| `fixings_missing` | A past fixing in a short-rate reference period was missing and filled from the curve |
| `carry_estimated` | The carry is a configured estimate |
| `carry_unobserved` | No carry was available: financing only |
| `anchor_interpolated` | The month lies between two forward nodes, one of them an anchor |
| `anchor_extrapolated` | The month lies beyond the anchor nodes |
| `anchor_stale` | The newest anchor print used is older than `max_age_hours` |
| `rolling` | The month is past its roll date (rank 0) |
| `expiring` | Less than one day to `expiry_ms` |

Flags are listed in this order, without duplicates.

## 10. Liquidity grade inheritance

- Each spot source's grade is its newest `liquidity_grade` record at or
  before `as_of_ms`; an ungraded source counts as `D` and as a `tape`.
- With two or more sources used, the output grade is the **second best**
  of their grades, so no single source decides. With one, it is that
  source's grade demoted one step (`A`→`B`, `B`→`C`, `C`→`D`). With a
  stale spot it is `D`.
- The label is `benchmark` for `A` and `B`, `tape` otherwise.
- Months without a traded spot input (short-rate months; energy months
  valued from anchors alone) have grade `null` and label `tape`.

## 11. Fair value: equity index, metals, energy

Let `lg(t) = z(t)·t` and let `c` be the carry (fraction, continuous).

### 11.1 Cost of carry

With the spot node `(t_s, S)`:

```text
F(τ) = S · exp( lg(τ) - lg(t_s) - c · (τ - t_s) )
```

For a cash spot this is `F = S · exp((r - c) · τ)` with `r = z(τ)`:

- **Equity index**: `c = q`, the dividend yield.
- **Gold and silver**: `c = l`, the lease rate.
- **Energy without anchors**: `c` is a configured convenience yield if
  any, else 0.

Carry precedence: an observed `series` (newest value; `carry_source`
`observed`), else the `estimate` (`estimated`, flag `carry_estimated`),
else none (`none`, `c = 0`, flag `carry_unobserved`, financing only).
The month a front-month tracker tracks is the spot: no carry applies and
no carry flag is raised.

### 11.2 Swap-print forward anchors

Where an exposure has `anchors` (crude oil and natural gas calendar-month
swaps in the example registry), commodity swap prints anchor the forward
curve. They are admitted, independent transaction prices.

- Matching prints: the configured venue and exact `underlier`, asset
  class `commodity`, price type `price`, an `expiry` date, tenor absent
  or `1M`, actions `new` or `correct`, event type `trade` or absent,
  cancelled prints removed, executed within the trailing `window_hours`.
- The pricing month of a print is the month of its `expiry`. Its anchor
  is the median price (times `multiplier`) of the month's prints, placed
  at the middle of the pricing month (UTC midnight of its first day plus
  half the month's length). Months whose middle has passed are dropped.
- Forward nodes are the spot node and the anchor nodes. Between nodes,
  `ln F` is linear in `t`; outside them, the nearest node is carried with
  the cost-of-carry drift of § 11.1. A month exactly at a node takes the
  node's price.
- A month valued from an anchor reports `carry_source` `swap_anchor`,
  `anchors` (the number of anchor nodes) and the implied carry from the
  reference node (the spot node, else the first anchor) to the month:
  `c = (lg(τ) - lg(t_ref) - ln(F / F_ref)) / (τ - t_ref)`, or across the
  nearest other node when the two coincide.

A calendar-month swap settles on the average of front-month futures
prices over its pricing month, which is mostly the next contract. The
method places both on one calendar-time forward curve (anchors at
mid-month, futures months at their expiry instants) rather than modelling
that average; see § 15.

## 12. Fair value: short-rate months

The fair value is `100 - 100 · R`, with `R` the implied rate over the
reference period `[start, end)`:

- `compounded_quarter`: from the third Wednesday of the contract month to
  the third Wednesday three months later;
  `R = (Π (1 + r_b · d_b / 360) - 1) · 360 / D`.
- `average_month`: the calendar days of the contract month;
  `R = Σ r_b · d_b / D`.

Here `D` is the period's calendar days and the product or sum runs over
the `us_government_securities` business days `b` from the business day on
or before `start` to the last before `end`; `d_b` is the number of the
period's calendar days from `b` (or `start`) to the next business day (or
`end`), so weekends and holidays carry the preceding business day's rate.
`r_b` is:

- for `b` before `today`: the fixing published for `b` (newest revision);
  if missing, the curve forward below, flagged `fixings_missing`;
- otherwise the curve's simple act/360 forward from `b` to the next
  business day: `r_b = (exp(lg(h2) - lg(h1)) - 1) · 360 / n` with
  `h1 = (b - today) / 365`, `n` the days to the next business day and
  `h2 = h1 + n / 365`.

No convexity adjustment is applied (§ 15). Inputs: the curve inputs and
the fixings used. `implied_rate` is `100 · R`; `spot` is null and `grade`
is null.

## 13. Treasury note and bond months (specified for v2)

Not in v1. The intended v2 method: for each deliverable issue (from
public auction results), the forward price from the government curve to
the delivery month, divided by the public conversion factor (price at a
6% yield, the published formula); the cheapest to deliver is the issue
with the lowest forward price over conversion factor, and the fair value
is that ratio, with no delivery-option value. It needs admitted auction
and outstanding-issue data, which are not yet admitted.

## 14. Records and positioning

- One record per listed month per valuation: instrument
  `synthetic:month:<exposure>:<YYYY-MM>`, venue `synthetic`.
- `prov.ts_source_ms` is the newest input's time, `prov.ts_ingest_ns` the
  newest input's ingest time; `prov.admission` is the lowest-numbered
  input admission and `admissions` lists them all; `prov.flags` adds
  `backfill` and `replayed` if any input carries them.
- `derived.inputs_hash` is the content hash (observation-v0 § 6.2) of the
  input records in canonical order without duplicates, and
  `derived.input_count` their number.
- `prov.seq` follows observation-v0 § 2.8. The standalone engine (the
  `synthetic` binary) runs on a fixed grid (`--step-secs`, default 60)
  with no blocks: each valuation at `t` sees exactly the inputs with
  `ts_source_ms <= t` and is positioned at `(floor(t / 1000), 0)`.
- A valuation never uses an input newer than `as_of_ms`.

## 15. Approximations and limits (v1)

- **Spot proxies.** Venue markets (for example perpetuals) track the
  underlying or the front month with a premium the method does not
  correct (funding and oracle fields are excluded by admission).
- **Sticky-tenor curve.** No roll-down between curve publications.
- **One curve for all exposures.** The curve's priority sets decide its
  basis (overnight-index swaps first, then government yields beyond);
  government yields differ from overnight-index rates by a spread the
  method does not model.
- **Day counts.** Act/365F horizons for all exposures; act/act and 30/360
  single periods are approximated (§ 7.1).
- **Par bootstrap.** Linear par interpolation between published tenors.
- **No convexity adjustment** for short-rate months; it grows with
  horizon, so back months are biased.
- **Swap anchors** at mid-month on a calendar-time curve, not as averages
  of contract prices.
- **Calendars.** Exchange early closes are not modelled; unscheduled
  closures need configuration.
- **No delivery options**, no Treasury months (§ 13).

## 16. Worked example

Made-up numbers; the arithmetic follows §§ 3–11.

**Spot composite** (`max_dispersion_bps` 100). Sources: A at 5000.00
(grade A), B at 5000.75 (grade B), C at 5150.00 (grade B), all fresh and
benchmarks. Median 5000.75; C is 298 bps away and is dropped; the median
of A and B is 5000.375, so `spot` = 5000.38 at scale 2. Flags:
`spot_dispersed`. Grade: second best of {A, B} = B, label `benchmark`.

**Equity index month.** Valuation 2026-03-12 15:00 UTC; month `2026-06`,
last trading day 2026-06-18 (the third Friday, 19 June, is Juneteenth),
expiry 09:30 New York = 13:30 UTC. `τ` = 97.9375 days / 365 =
0.26832192. Curve nodes `z(0.25)` = 4.10%, `z(0.50)` = 4.00%:
`lg(τ) = 0.01025 + (0.26832192 - 0.25)/0.25 · (0.02 - 0.01025)` =
0.01096455, so `financing_rate` = 4.086343. Spot 5000.00, estimated
dividend yield 1.30%:

```text
F = 5000.00 · exp(0.01096455 - 0.013 · 0.26832192) = 5037.52
```

`carry_source` `estimated`, flag `carry_estimated`.

**Crude month between anchors.** A front-month spot node at `t_s` = 0.02
at 70.00; anchors at `t` = 0.09 (69.60) and 0.17 (69.10). A month at
`τ` = 0.13 lies halfway: `F = sqrt(69.60 · 69.10)` = 69.35, flag
`anchor_interpolated`. With a flat 4% curve, the implied carry from the
spot node is `0.04 - ln(69.3495 / 70.00) / 0.11` = 12.484928%.

**One-month rate.** A 30-day month with 10 days fixed at 4.30% and 20
days of curve forwards at 4.05%: `R` = (10 · 4.30 + 20 · 4.05) / 30 =
4.133333%, `fair_value` = 95.8667 at scale 4.

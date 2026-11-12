# synthetic

Synthetic contract months: a fair-value series for each contract month of
an exposure, computed without exchange data by the method
`synthetic-months@1`.

**Status:** library and binary implemented and tested on synthetic
fixtures only. The reference sources (government yields, the overnight
rate) are being admitted; until their pollers exist the engine has no real
rate inputs. Not yet wired into the origin. **Owner:** synthetic
contract-month lane.

## Responsibility

- Read mesh records: venue `trade`, `book_snapshot`, `queue_positions`
  and `liquidity_grade` records (spot), `reference_value` records (rates,
  carry) and `swap_print` records (overnight-index swaps, commodity
  forward anchors).
- Compute US business-day calendars, last trading days and roll dates
  from rulebook rules; build the rate curve; form a spot composite that
  tolerates the loss of any one source; value every listed month per
  family; flag quality and inherit a liquidity grade.
- Emit `synthetic_month` records with an inputs hash, reproducible bit
  for bit (deterministic `exp` and `ln`, fixed rounding).

Method: [docs/specs/method-synthetic-months-v1.md](../../docs/specs/method-synthetic-months-v1.md).
Decision: [ADR 0013](../../docs/decisions/0013-synthetic-contract-months.md).

## Families

| Family | v1 |
|---|---|
| Equity index (quarterly) | Implemented: `F = S·exp((r − q)·τ)` |
| Crude oil, natural gas (monthly) | Implemented: swap-print anchors, else financing only (`carry_unobserved`) |
| Gold, silver | Implemented: `F = S·exp((r − l)·τ)` |
| One- and three-month overnight-rate months | Implemented: average or compounded rate from fixings and curve forwards |
| Treasury notes and bonds | Specified for v2 only |

## Use

```bash
cargo run -p synthetic -- \
  --registry fixtures/synthetic/synthetic-months/registry.json \
  --input fixtures/synthetic/synthetic-months/inputs.jsonl \
  --step-secs 3600 > months.jsonl
```

The registry in the repository has synthetic example entries only; real
venue market IDs are added by the operator.

## Modules

`detmath` (deterministic `exp`, `ln`), `date` (dates, New York time),
`calendar` (holidays, expiry and roll rules), `curve` (quotes, par
bootstrap, interpolation), `spot` (composite, grade inheritance), `fair`
(families), `registry` (JSON registry), `engine` (state and valuation),
`batch` (JSON lines and grid runs).

## Tests

All on synthetic data: calendar edge cases, convergence to spot at expiry,
continuity across rolls, swap-anchor interpolation, composite robustness
to removing one source, staleness flags, short-rate fixings, no future
data, determinism, and a golden output for the committed fixture
(`SYNTHETIC_REGENERATE=1 cargo test -p synthetic --test fixtures`
rewrites it after an intended change).

## Rules

- Owned by one lane; shared types come from `mesh-types` (integrator).
- No exchange data in code, fixtures or tests. No real venue market IDs
  in the repository's registry.
- See [ADR 0007](../../docs/decisions/0007-code-layout.md).

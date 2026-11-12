# Synthetic contract-month fixtures

**Everything here is synthetic.** No value comes from an exchange, a
vendor, a venue or any real market or publication. The venues
(`example-a`, `example-b`, `example-c`, `example-rates`, `example-swaps`,
`sdr-example`), markets, prices, rates, swap prints and admission IDs
(`ADM-9001`, `ADM-9002`, `ADM-9003`, which name no admission record) are
invented. Calendar rules in the registry are published rulebook rules,
not data.

| File | What |
|---|---|
| `registry.json` | Exposure registry with synthetic example entries for every v1 family |
| `inputs.jsonl` | 12 days (2026-03-09 to 2026-03-21) of invented spot trades on eight markets, liquidity grades, an overnight rate with history and one revision, bill and par curves, overnight-index swap prints, crude swap anchors and one cancelled print |
| `expected.jsonl` | `synthetic_month` records from the engine on a daily grid at 20:00 UTC |

The inputs exercise: the equity index roll (10 March) and expiry
(20 March), the crude roll (17 March) with a front-month tracker, an index
outlier dropped from the composite, a single tape gas source, a silver
market that stops trading (stale spot, then no value), a three-month rate
quarter ending (17 March) and a one-month rate month in progress.

## Regenerate

`crates/synthetic/tests/common/mod.rs` generates `inputs.jsonl`
deterministically; `cargo test -p synthetic` fails if either file is
stale. After an intended change:

```bash
SYNTHETIC_REGENERATE=1 cargo test -p synthetic --test fixtures
```

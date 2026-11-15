# Synthetic reference-source fixtures

**Everything here is synthetic.** Every value, date, market name, code,
CUSIP and document ID is invented; nothing comes from a real publication,
an exchange or a vendor. The files only mimic the documented layouts in
[reference-sources.md](../../../docs/specs/reference-sources.md), so the
readers in `crates/reference-pollers` can be tested without the network.

| Directory | Mimics | Edge cases |
|---|---|---|
| `nyfed/` | Markets Data API `refRates` JSON | An undocumented key, a malformed record, a same-day revision (`secured_all_revised.json`) |
| `treasury/` | home.treasury.gov OData Atom feeds | A new tenor (`BC_9MONTH`), a missing element, `CS_*` averages (not admitted), a malformed value, composite rows without a tenor |
| `fiscaldata/` | `auctions_query` pages | `links.next` paging, a TIPS row, a malformed row, amounts (not emitted) |
| `frb/` | H.15 and H.10 SDMX data files | Excluded vendor series, an unknown series, `ND`, a weekly series, a revision |
| `cot/` | COT annual (with header) and weekly (without) files | Legacy spaced headers, doubled underscores, a misspelt supplemental column, a malformed count, a December report published in January |
| `lmr/` | LMPR DataMart sections | Every purchase type (excluded ones included), an unknown type, a correction |
| `dairy/` | NDPSR and class-price sections | `Final` sections (revisions), an exchange-named field, an undocumented price |
| `feeder/` | Fixed-layout weekly summary text | Narrative quoting futures, basis and futures rows, a note column, a stale issue |
| `ercot/` | Report listings and CSV documents | XML duplicates, an invalid hour, the repeated hour at the end of daylight time, a correction, an unknown column |
| `eia/` | EIA bulk files (`PET.txt`, `NG.txt`; zipped in the tests) | Excluded spot, futures, Henry Hub and wholesale series labelled as EIA's own, an allowlisted price and an allowlisted volume with a vendor credit, a price in wrong units, volumes of every measure (stocks with Cushing, flows, capacity, utilisation, gas storage, monthly flows; ADR 0023), a stock stated per day and an undocumented unit text, `null`, `NA` and `W` values, a series not on the allowlist, category lines |
| `bls/` | BLS time-series data files (`cu`, `cw`, `su`, `ap`), tab-separated and padded | Annual and semiannual periods, a gap (footnote `X`), a PPI series, malformed rows, a non-default area, a seasonal revision and C-CPI-U initial to interim (`.revised`) |

ERCOT historical workbooks and zip packaging are built in the tests.

## Regenerate

```bash
python3 fixtures/synthetic/reference/generate.py .
```

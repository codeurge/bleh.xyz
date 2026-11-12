# 0015: Data model amendments for the reference pollers

- Status: Proposed
- Date: 2026-11-12
- Deciders: Repository owner (proposed by the reference-pollers lane, which
  owned `crates/mesh-types` for this change)

## Context

The reference pollers (task 4 of the
[open reference prices plan](../../.agents/plans/open-reference-prices.md))
emit records for the sources admitted in ADM-0005 to ADM-0013. Writing them
against the v0 model surfaced four gaps, each of which changes the data
model or the manifest format and so needs a record
([AGENTS.md](../../AGENTS.md) rule 4):

- [ADR 0012](0012-open-terms-route-for-public-bodies-and-market-operators.md)
  decision 6 created the `exchange_referenced` flag and assigned it to the
  poller lane as the next free bit. Dairy prices, swine formula prices and
  combined Commitments of Traders figures must carry it.
- Commitments of Traders figures are positions, not prices.
  `reference_value` holds one decimal value with rate or price
  conventions; forcing long, short, spreading and open interest into it
  would need one record per number, a measure that is not a measure, and
  would lose the category and report family.
- Grid operators publish one price per hour or per 15-minute interval of a
  delivery day. `reference_value` carries only `as_of_date`, so intervals
  of the same day could not be told apart.
- ADR 0012 decision 5 requires notices (for example the New York Fed's
  reference-rates notice) to travel in "the archive manifest", which had
  no place for them.

`observation-v0` and `bleh-manifest-v0` are drafts, so these are
amendments of v0, as in [ADR 0011](0011-v0-amendments-for-the-origin.md).

## Decision

Observation v0 ([observation-v0.md](../specs/observation-v0.md)):

1. **Flag bit 7 `exchange_referenced`** (§ 2.9), with the meaning given
   by ADR 0012 decision 6.
2. **A non-derived `positioning` kind** (§ 2.14): report family
   (`legacy`, `disaggregated`, `tff`, `supplemental`), `combined`,
   `contract_market_code`, `as_of_date`, trader `category`, `long`,
   `short`, `spreading` (nullable), `open_interest` and `revision`, on
   instrument `<source>:positioning:<contract_market_code>`. It is flat in
   Arrow like `reference_value`. Validation requires `exchange_referenced`
   exactly on combined records and the category to exist in the family.
3. **Optional `period_start_ms` and `period_end_ms` on `reference_value`**
   (§ 2.12): the intra-day delivery or settlement interval a value is for.
   Both or neither; omitted rather than `null` when absent, so the
   canonical encoding of every existing `reference_value` record is
   unchanged. The Arrow schema gains two nullable timestamp columns.

Manifest v0 ([manifest-v0.md](../specs/manifest-v0.md)):

4. **An optional `notices` list** (§ 2.3) of `{admission, source,
   notice}`, sorted and unique, omitted when empty, so existing manifests
   keep their bytes and signatures. The archive's manifest builder reads it
   from `_notices.json` at the archive root, which the reference pollers
   write. The pollers also publish the notices in their series catalogue
   (the registry entry for reference series).

Canonical encodings of existing kinds are unchanged and the fast encoder
is untouched (it covers trades, book events and snapshots only).

## Consequences

- Dairy, swine formula and combined COT records can be emitted, flagged
  as ADR 0012 requires.
- COT figures are queryable per market, family, form and category, and
  carry no price.
- Hourly and 15-minute grid prices are distinct records with exact
  interval bounds in UTC.
- Every manifest of an archive holding NY Fed records carries the notice,
  signed. A release that drops a required notice is a defect (ADR 0012).
- Readers of the previous v0 draft must accept bit 7, the new kind, the
  two optional fields and the manifest list (unknown fields are
  rejected, so old readers fail loudly rather than misread).

## Alternatives considered

- **COT in `reference_value`:** one record per number with an
  `index_level` measure; loses structure and misuses the measure.
- **Encoding the interval in the instrument symbol** (`...:HB_X.HE01`):
  24 to 96 instruments per settlement point, and no exact UTC bounds.
- **A `NOTICE` data file in each day's tree:** listed and hashed, but not
  in the signed body; consumers would have to fetch and match it.
- **Notices in every record:** repeats long text on millions of records
  and changes their hashes.

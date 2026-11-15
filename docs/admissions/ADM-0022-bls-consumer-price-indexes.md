# ADM-0022: BLS — consumer price indexes (CPI-U, CPI-W, C-CPI-U, average prices)

- Status: Admitted
- Requested: 2026-11-14 by the research lane
  (open sources survey,
  `docs/research/open-sources-survey-2026-11.md`, rank 3)
- Reviewers: Repository owner (engineering review of field lineage)
- Decided: 2026-11-15 by the repository owner (owner decision, 2026-11-15)

## Source

The US Bureau of Labor Statistics (BLS) publishes the Consumer Price Index
every month, usually at 8:30 a.m. ET. BLS staff collect prices from retail
outlets, service providers and housing units, and BLS computes the indexes
itself.

### Uses

- **TIPS:** the non-seasonally-adjusted CPI-U (all items, US city average)
  is the index that Treasury Inflation-Protected Securities accrue against,
  through the reference CPI.
- **Inflation swaps and listed inflation products:** they refer to the same
  index.

### Channels

- **Bulk time-series files (primary):**
  `https://download.bls.gov/pub/time.series/cu/` for CPI-U,
  `.../cw/` for CPI-W, `.../su/` for C-CPI-U and `.../ap/` for average
  prices.
  - BLS refuses generic clients and asks bulk users to send a User-Agent
    with a contact address. That address must be a project role address
    (owner decision), never a personal one.
- **Public Data API (secondary, for checks):** v2 needs a free key,
  registered with an email address and organisation and renewed yearly.
  Unregistered v1 is enough for a few series.

**History:** CPI-U from 1913 (US city average, all items); CPI-W from 1913;
C-CPI-U from 1999.

## Fields requested

| Field | Meaning | Admit? |
|---|---|---|
| CPI-U index values, not seasonally adjusted and seasonally adjusted, all items and published components and areas | The main index, by item and area | Requested |
| CPI-W index values, same breakdown | Index for wage earners | Requested |
| C-CPI-U index values, initial and final | Chained index, with later revisions | Requested |
| Average price data (`ap` series) | BLS's own average retail prices for selected items | Requested |
| Series metadata: series ID, item, area, base period, seasonal flag; period; footnote codes; release date | Identity and timing | Requested |
| Revisions: seasonally adjusted series revised each year; C-CPI-U revised from initial to final | Revision markers | Requested, as revisions with `as_of_date` |
| Producer price indexes, import and export price indexes | Different programmes. Some PPI farm and energy indexes start from exchange quotes or purchased datasets; import fuel indexes use spot prices from an unnamed source | Not requested. Each needs its own review |
| BLS photographs, illustrations, emblem and logo | Excluded from the public domain or restricted | Not requested |

## Policy checks

Under [admission-policy.md](../admission-policy.md):

1. **Original price formation.** CPI is computed by BLS from prices that
   BLS collects. It is a public statistic, like the admitted USDA and
   Treasury series, and no exchange price is an input.
   - The CPI gasoline component uses a secondary dataset of retail pump
     prices. That is not an exchange or exchange-vendor price, so it passes.
2. **No contract on the path.** Public-domain government data.
   - The BLS [copyright page](https://www.bls.gov/opub/copyright-information.htm)
     says BLS material "is in the public domain, except for previously
     copyrighted photographs and illustrations", and "You are free to use
     our public domain material without specific permission". BLS asks to
     be cited as the source.
   - The [API terms](https://www.bls.gov/developers/termsOfService.htm)
     say: "Data accessed through BLS.gov do not, and should not, include
     controls over its end use." They ask users to cite the retrieval date
     and to state: "BLS.gov cannot vouch for the data or analyses derived
     from these data after the data have been retrieved from BLS.gov." They
     also forbid modifying or misrepresenting content while citing BLS.
   - There is no limit on recipients and no ban on commercial use or
     redistribution. The key passes the test in rule 2.
   - The citation, retrieval date and disclaimer are carried as this
     source's notice, in the registry entry and the archive manifest, as
     for other notice-bearing sources
     ([terms.md](../terms.md) § 3).
3. **Venue-matched fields only.** The admitted fields are BLS's published
   index values and average prices, unmodified.
4. **Field-level exclusions.** PPI and import/export indexes are not
   requested, and BLS images and emblems are excluded.

## Source basis

Read 2026-11-14:

- <https://www.bls.gov/opub/copyright-information.htm>
- <https://www.bls.gov/developers/termsOfService.htm>
- <https://download.bls.gov/pub/time.series/cu/> (listing; needs an
  identifying User-Agent)

## Related change: TIPS index ratios

Fiscal Data publishes Treasury's "TIPS and CPI Data": the reference CPI and
the daily index ratio for each TIPS issue. It is a Fiscal Data dataset, the
channel ADM-0006 already admits. So it would be added by amending ADM-0006,
not by this record. The endpoint paths need checking when it is wired up.

## Accuracy value

- **Settlement anchor:** CPI-U is the anchor for US inflation products.
  With the TIPS index ratios, it gives real-yield carry for Treasury months
  (method v2).
- **Integrity:** a cross-check for breakeven rates implied by Treasury
  nominal and real curves (ADM-0006).

## Risks and review triggers

- BLS changes its copyright page or API terms.
- BLS starts building CPI components from exchange or exchange-vendor
  prices.
- Release schedule changes, or publication is suspended (for example
  during a government shutdown). Gaps are flagged, never filled.

## Decision

Admitted on 2026-11-15 by the repository owner for the fields above, read from BLS
bulk files (API for checks).

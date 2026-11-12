# ADM-0009: Federal Reserve Board — H.10 Foreign Exchange Rates

- Status: Admitted
- Requested: 2026-11-12 by the admissions lane (open reference prices plan, task 1)
- Reviewers: Repository owner (engineering review of field lineage)
- Decided: 2026-11-12 by the repository owner (owner decision, 2026-11-12)

## Source

The Board of Governors of the Federal Reserve System publishes the H.10
release each week. It contains daily rates of exchange for 23 currencies
against the US dollar, plus trade-weighted dollar indexes.

The [H.10 page](https://www.federalreserve.gov/releases/h10/about.htm)
(read 2026-11-12) says: "The data are noon buying rates in New York for
cable transfers payable in the listed currencies. The rates have been
certified by the Federal Reserve Bank of New York for customs purposes as
required by section 522 of the amended Tariff Act of 1930."

The [H.10 historical data page](https://www.federalreserve.gov/releases/h10/hist/)
(read 2026-11-12) adds: "The information is based on data collected by the
Federal Reserve Bank of New York from a sample of market participants."

We read the whole-release XML file linked from the
[H.10 release page](https://www.federalreserve.gov/releases/h10/current/).
Endpoints are in
[reference-sources.md](../specs/reference-sources.md#federal-reserve-board-h15-and-h10).

## Fields

| Series | Meaning | Admit? |
|---|---|---|
| `RXI$US_N.B.*` (AUD, EUR, NZD, GBP) and `RXI_N.B.*` (BRL, CAD, CNY, DKK, HKD, INR, JPY, MYR, MXN, NOK, SGD, ZAR, KRW, LKR, SEK, CHF, TWD, THB, VEB) | Certified noon buying rates, daily, from 1971 for the oldest series | **Admitted** |
| `JRXWTFB_N`, `JRXWTFN_N`, `JRXWTFO_N` nominal Broad, AFE and EME dollar indexes; `V0.*` goods-only indexes; real indexes | Trade-weighted indexes | **Excluded.** See below |

Why the indexes are excluded:

- The Broad index and its subsets use currencies that are not in the H.10
  table.
- Earlier H.10 releases named the source of those rates. For example, the
  [24 October 2005 release](https://www.federalreserve.gov/releases/h10/20051024/h10.htm)
  says: "The source for exchange rates not listed in the table above but
  used in the calculation of the broad and OITP indexes is Reuters
  Limited."
- The current release does not state the source. The indexes may therefore
  embed commercial-vendor prices.

## Policy checks

Under [admission-policy.md](../admission-policy.md):

1. **Original price formation.** The bilateral rates are the New York Fed's
   certification of rates "collected ... from a sample of market
   participants". That means quotes reported to a public body, which
   certifies them under statute. There is no exchange in the spot FX
   market these rates describe.
2. **No contract on the path.** The data are public-domain government data
   under the Board's
   [website policies](https://www.federalreserve.gov/disclaimer.htm) ("in
   the public domain and may be copied and distributed without permission.
   Please cite to the Board").
3. **Venue-matched fields only.** The admitted rates are the NY Fed's
   certified statistics. The indexes are excluded because their inputs may
   include vendor rates.
4. **Field-level exclusions.** The dollar indexes, as above.

## Source basis

Public-domain publication by a federal agency, read 2026-11-12:

- <https://www.federalreserve.gov/releases/h10/about.htm>
- <https://www.federalreserve.gov/releases/h10/hist/>
- <https://www.federalreserve.gov/releases/h10/data/FRB_h10_xml.zip>
- <https://www.federalreserve.gov/disclaimer.htm>

## Risks and review triggers

- The NY Fed or the Board documents that the noon rates are taken from a
  vendor or an exchange-group platform. This record would then be revoked.
- The Board names an open source for the non-table currencies, which would
  allow the indexes to be reviewed again.
- DDP retirement changes the file's location.

## Decision

Admitted on 2026-11-12 by the repository owner for the certified bilateral
noon buying rates, read from the Board's whole-release XML file. The dollar
indexes are excluded.

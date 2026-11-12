# ADM-0008: Federal Reserve Board — H.15 Selected Interest Rates

- Status: Admitted
- Requested: 2026-11-12 by the admissions lane (open reference prices plan, task 1)
- Reviewers: Repository owner (engineering review of field lineage)
- Decided: 2026-11-12 by the repository owner (owner decision, 2026-11-12)

## Source

The Board of Governors of the Federal Reserve System, a federal agency,
publishes the H.15 Selected Interest Rates release each business day. It
keeps current and discontinued series in its Data Download Program (DDP).

The Board is retiring the DDP. It removed the "Build Your Package" option
the week of 2026-11-09 and says "Historical data will remain available for
download as XML files on statistical release pages". We therefore read the
whole-release SDMX XML file linked from the
[H.15 release page](https://www.federalreserve.gov/releases/h15/).
Endpoints are in
[reference-sources.md](../specs/reference-sources.md#federal-reserve-board-h15-and-h10).

The Board names the sources of the discontinued third-party series on its
[H.15 technical Q&A](https://www.federalreserve.gov/releases/h15/h15_technical_qa.htm)
(read 2026-11-12): "Eurodollar deposits: Bloomberg and ICAP; interest rate
swaps: ICE Benchmark Administration; corporate bonds: Moody's; state and
local bonds: Bond Buyer Index; and conventional mortgages: Freddie Mac."

## Fields

Admission is series by series. The series names are the DDP `SERIES_NAME`
stems, all frequencies included.

| Series | Source as documented by the Board | Admit? |
|---|---|---|
| `RIFSPFF_N` federal funds (effective), 1954 on | NY Fed; FR 2420 transactions since 2016-03-01, brokered trades before | **Admitted.** Records carry the NY Fed reference-rates notice from [ADM-0007](ADM-0007-nyfed-reference-rates.md), because the NY Fed administers EFFR |
| `RIFSPPNAAD*`, `RIFSPPFAAD*` nonfinancial and financial AA commercial paper (1, 2, 3 months), 1997 on | "interpolated from data on certain commercial paper trades settled by The Depository Trust Company" | **Admitted** |
| `RIFSPBLP_N` bank prime loan | "Rate posted by a majority of top 25 ... insured U.S.-chartered commercial banks" | **Admitted** |
| `RIFSRP_F02_N` discount window primary credit; `H0.RIFSRD_F02_N` historical discount rate | Federal Reserve's own administered rate | **Admitted** |
| `RIFSGFS*` Treasury bills, secondary market (4 weeks to 1 year) | Board series, not marked as third-party | **Admitted** |
| `RIFLGFC*` Treasury constant maturities, nominal and inflation-indexed; `RIFLGFL_XII_N` inflation-indexed long-term average | "Source: U.S. Treasury" | **Admitted** (same lineage as [ADM-0006](ADM-0006-us-treasury-rates-and-auctions.md)) |
| `H0.RIFLGFCY20_N`, `H0.RIFLGFL_N`, `H1.RIFLGFY10P_N` historical 20-year CMT, long-term and composite Treasury yields | US Treasury | **Admitted** |
| `H1.RIFSGFPAM03_N`, `H0.RIFSGFPAM06_N`, `H0.RIFSGFPAY01_N` Treasury bill auction high rates | Treasury auctions | **Admitted** |
| `RIFSPPCU_N`, `RIFSPPCUS_N` Commercial Paper Funding Facility posted rates | Federal Reserve facility rates | **Admitted** |
| `RILSPDEP*` Eurodollar deposits | Bloomberg and ICAP | **Excluded.** Commercial vendor and broker |
| `RIFLDIY*` interest rate swaps | ICE Benchmark Administration | **Excluded.** Exchange-group benchmark administrator |
| `RIMLPAAAR_N`, `RIMLPBAAR_N` Moody's Aaa and Baa | Moody's | **Excluded.** Commercial vendor |
| State and local bonds | Bond Buyer Index | **Excluded.** Commercial vendor |
| Conventional mortgages | Freddie Mac | **Excluded.** Third-party survey with its own terms |
| `H0.RIFSPDCNSM*` negotiable CDs (secondary market); `H0.RIFSPPCM*` dealer prime commercial paper; `H0.RIFSPPFM*` finance paper placed directly; `H0.RIFSPABM*` / `H1.RIFSPABM*` bankers' acceptances | Discontinued; the Board documents no source | **Excluded.** Lineage not documented |
| Any series added later | — | Not covered until reviewed |

## Policy checks

Under [admission-policy.md](../admission-policy.md):

1. **Original price formation.** Each admitted series is one of:
   - the Federal Reserve's own administered rate;
   - a statistic computed from transactions or posted rates reported to it
     (FR 2420, DTC-settled commercial paper, banks' posted prime rates);
   - US Treasury data or Treasury auction results.

   The excluded series are the ones the Board sourced from vendors and
   benchmark administrators, or whose source is undocumented.
2. **No contract on the path.** The admitted series are public-domain
   government data. The Board's
   [website policies](https://www.federalreserve.gov/disclaimer.htm) (read
   2026-11-12) say: "Unless otherwise indicated, information on Board's
   website is in the public domain and may be copied and distributed without
   permission. Please cite to the Board as the source of the information."
   - The third-party series are "otherwise indicated" on the technical Q&A,
     so they are excluded.
   - EFFR is administered by the NY Fed, so its records also carry the
     NY Fed notice. This is a conservative choice; the Board publishes the
     series without a third-party marking.
3. **Venue-matched fields only.** No admitted series comes from an exchange
   feed, an oracle or a commercial vendor.
4. **Field-level exclusions.** As listed in the table.

## Source basis

Public-domain publication by a federal agency, read 2026-11-12:

- <https://www.federalreserve.gov/releases/h15/>
- <https://www.federalreserve.gov/releases/h15/data/FRB_h15_xml.zip>
- <https://www.federalreserve.gov/disclaimer.htm>
- <https://www.federalreserve.gov/data/data-download-fred-information.htm>

## Risks and review triggers

- The Board adds a series, or marks an admitted series as third-party.
- The DDP retirement changes the XML file's location or structure.
- FRED is not a channel. It is operated by the Federal Reserve Bank of
  St. Louis under its own terms, which this record does not review.

## Decision

Admitted on 2026-11-12 by the repository owner for the admitted series
above, read from the Board's whole-release XML file. Vendor-sourced and
undocumented series are excluded.

# ADM-0013: ERCOT — day-ahead and real-time settlement point prices, LMPs and ancillary service clearing prices

- Status: Admitted
- Requested: 2026-11-12 by the admissions lane (open reference prices plan, task 1)
- Reviewers: Repository owner (engineering review of field lineage)
- Decided: 2026-11-12 by the repository owner (owner decision, 2026-11-12)

## Source

The Electric Reliability Council of Texas (ERCOT) runs the ERCOT wholesale
market under the Public Utility Commission of Texas (PUCT). ERCOT clears a
day-ahead market and a real-time market (SCED), and it posts the following:

- settlement point prices (SPPs) for hubs, load zones and resource nodes;
- LMPs by electrical bus and settlement point;
- ancillary service market clearing prices for capacity (MCPCs).

These postings are required by the Nodal Protocols:

- Section 4, § 4.5.2(2) for day-ahead results ("no later than 1330");
- Section 6, § 6.3.2(2) for real-time results.

Source: <https://www.ercot.com/mktrules/nprotocols/current>, read
2026-11-12. The data products are classified "Public" in ERCOT's Market
Information List (EMIL).

We read the public report listings and downloads on www.ercot.com, which
need no account. Endpoints are in
[reference-sources.md](../specs/reference-sources.md#ercot).

## Fields

| Product (EMIL ID) | Fields | Admit? |
|---|---|---|
| DAM Settlement Point Prices (NP4-190-CD) | `DeliveryDate`, `HourEnding`, `SettlementPoint`, `SettlementPointPrice`, `DSTFlag` | **Admitted** |
| DAM Hourly LMPs (NP4-183-CD) | `DeliveryDate`, `HourEnding`, `BusName`, `LMP`, `DSTFlag` | **Admitted** |
| DAM Clearing Prices for Capacity (NP4-188-CD) | `DeliveryDate`, `HourEnding`, `AncillaryType`, `MCPC`, `DSTFlag` | **Admitted** |
| Settlement Point Prices at Resource Nodes, Hubs and Load Zones, 15-minute (NP6-905-CD) | `DeliveryDate`, `DeliveryHour`, `DeliveryInterval`, `SettlementPointName`, `SettlementPointType`, `SettlementPointPrice`, `DSTFlag` | **Admitted** |
| LMPs by Resource Nodes, Load Zones and Trading Hubs, per SCED run (NP6-788-CD) | `SCEDTimestamp`, `RepeatedHourFlag`, `SettlementPoint`, `LMP` | **Admitted** |
| Real-time AS MCPCs, 15-minute (NP6-331-CD) and per SCED (NP6-332-CD) | `DeliveryDate`, `DeliveryHour`, `DeliveryInterval`, `RepeatedHourFlag`, `ASType`, `MCPC`; `SCEDTimestamp`, `CappedMCPC`, `UncappedMCPC` | **Admitted** |
| Historical DAM and RTM Load Zone and Hub Prices (NP4-180-ER, NP6-785-ER) | Annual workbooks of the same hub and load-zone SPPs, from 2010 and 2011 | **Admitted** |
| Anything obtained through the ERCOT Public API (api.ercot.com) | Same data, behind account terms | **Not used.** See check 2 |

## Policy checks

Under [admission-policy.md](../admission-policy.md):

1. **Original price formation.** The prices are cleared by ERCOT's own
   day-ahead and real-time market optimisation from participants' offers
   and bids. No outside price is an input.
2. **No contract on the path.** The open-terms route for regulated market
   operators applies
   ([ADR 0012](../decisions/0012-open-terms-route-for-public-bodies-and-market-operators.md)).
   The [ERCOT Website Terms of Use](https://www.ercot.com/help/terms) (last
   updated 07/20/2023, read 2026-11-12) say in § 5: "The publicly available
   contents of this website may be used, reproduced, and redistributed,
   provided that the contents are not modified and that you maintain all
   copyright and other notices contained in the contents, including this
   Agreement. Notwithstanding the foregoing, raw data provided in public
   portions of this website may be used, reproduced, and redistributed in
   compilations, charts, and analyses without maintaining such notices."
   - There is no commercial-use restriction.
   - § 9 reserves the ERCOT name and logo as trademarks. We use the name
     only to identify the source.
   - The [Terms of Use for the ERCOT API](https://www.ercot.com/help/terms/data-portal)
     require an account, forbid bot-registered accounts, and say: "ERCOT sets
     and enforces limits on your use of the ERCOT API (e.g. limiting the
     number of postings that you may make or the number of users you may
     serve), in ERCOT's sole discretion." A limit on users served fails the
     ADR 0012 test, so the API channel is not used. Website downloads need
     no account and fall under the website terms.
3. **Venue-matched fields only.** The fields are ERCOT's own cleared prices.
4. **Field-level exclusions.** None among the listed products. Pollers must
   not use api.ercot.com.

## Source basis

Postings required by the PUCT-approved Nodal Protocols, offered under the
ERCOT website terms, read 2026-11-12:

- <https://www.ercot.com/help/terms>
- <https://www.ercot.com/mktrules/nprotocols/current>
- <https://www.ercot.com/mp/data-products/data-product-details?id=NP6-905-CD>
  (and the other EMIL IDs above)

## Risks and review triggers

- ERCOT changes § 5 of its website terms. ERCOT reserves the right to
  change them "at any time without notice", so the terms are rechecked at
  each poller release.
- Website downloads start requiring an account.
- Market redesign changes product IDs or price definitions (for example the
  real-time co-optimisation products added in 2025).

## Decision

Admitted on 2026-11-12 by the repository owner for the products and fields
above, read from public www.ercot.com downloads, without the ERCOT API.

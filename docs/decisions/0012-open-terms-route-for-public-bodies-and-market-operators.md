# 0012: Admission policy amendment — open-terms route for public bodies and regulated market operators

- Status: Accepted
- Date: 2026-11-12
- Deciders: Repository owner (drafted by the admissions lane under the
  owner's delegation for the open reference prices plan, 2026-11-12)

## Context

Many cash-settled futures settle against data that public bodies and
regulated market operators publish: overnight reference rates, grid
operators' hub prices, USDA cash prices and survey prices. The
[open reference prices plan](../../.agents/plans/open-reference-prices.md)
asks the project to admit as many of these as clean lineage allows.

Rule 2 of the [admission policy](../admission-policy.md) lists four routes:

- our own node;
- an openly published bucket;
- data that law or regulation requires to be published free of charge;
- public-domain government data.

Two valuable sources fail rule 2's wording on a technicality, although
neither restricts redistribution:

- **Federal Reserve Bank of New York.** The NY Fed administers SOFR, EFFR,
  OBFR, TGCR, BGCR and the SOFR averages and index. It is not a federal
  agency, so its content is not public-domain government data. Its
  [Terms of Use](https://www.newyorkfed.org/privacy/termsofuse) (read
  2026-11-12) grant "a non-exclusive license, subject to the Terms, to use,
  copy, and distribute Content for your personal or business purposes". The
  conditions are a notice and disclaimer that must travel with
  reference-rate data, attribution, labelling of modifications, no implied
  endorsement, and passing the same terms on.
- **ERCOT.** ERCOT posts prices because the PUCT-approved Nodal Protocols
  require it. The protocols do not say "free of charge", so the free-by-law
  route does not literally apply. ERCOT's
  [website terms](https://www.ercot.com/help/terms) (§ 5, read 2026-11-12)
  say that "raw data provided in public portions of this website may be
  used, reproduced, and redistributed in compilations, charts, and analyses
  without maintaining such notices".

Other operators publish under terms that do restrict redistribution.
Examples are PJM's ban on republication by non-members and SPP's ban on
commercial use without written authorization. Any amendment must keep
those sources out.

Some admitted public statistics also come from transactions that may be
privately priced against an exchange price, even though they contain no
exchange price themselves. One example is dairy product sales reported to
USDA that may be priced by formula from an exchange's spot cash market.
Rule 3 already handles the venue case with a mechanism description and the
`oracle_anchored_venue` flag. These public-source records need the same
treatment.

## Decision

1. **Rule 2 gains a fifth route.** A source also qualifies if two things
   hold:
   - It is a public body (a government agency or a Federal Reserve Bank) or
     a regulated market operator (for example a grid operator publishing
     under a regulator-approved tariff or protocols).
   - It publishes the data openly, under terms that expressly let anyone
     copy and redistribute it, for any purpose including commercial use,
     free of charge.
2. **Acceptable conditions** are limited to:
   - attribution, or a source notice or disclaimer that travels with the
     data;
   - passing the same terms on;
   - not implying endorsement;
   - labelling modified data, and not altering data that is presented as
     the source's own.
3. **Disqualifying conditions:**
   - any limit on who may receive the data or how many users may be served;
   - a ban on commercial use or republication;
   - a fee, membership or written permission requirement;
   - a reservation that leaves redistribution to the publisher's
     discretion.

   Silence is not permission. A site that reserves its rights without
   granting redistribution does not qualify under this route.
4. **Registration.** A free registration or API key is acceptable only when
   its own terms pass the same test. Otherwise the poller uses an
   unregistered channel, or the channel is not used.
5. **Notices travel with the data.** For records admitted under this route,
   the source's required notice is carried in the registry entry and the
   archive manifest. CC0 applies to the project's own contribution, and the
   source's notice conditions pass through to users for those records.
   This narrows the [ADR 0002](0002-licensing.md) consequence "downstream
   users carry no attribution or share-alike obligations for data" for
   these records only.
6. **New record flag `exchange_referenced`.** It applies to a statistic
   from an admitted public source that meets either condition:
   - Its underlying transactions may be privately priced by formula against
     an exchange price or an exchange-published index.
   - Its computation uses a factor an exchange supplies.

   Each admission record describes the mechanism. Every affected record
   carries the flag, by analogy with `oracle_anchored_venue` under rule 3.
   The record is still the public body's statistic, never an exchange
   price. The poller lane adds the flag as the next free bit (bit 7) in a
   minor version of the observation spec before emitting such records.

Rules 1 and 3 are unchanged. No exchange-derived price is admitted, and no
source whose terms restrict redistribution is admitted.

## Consequences

- The NY Fed reference rates (ADM-0007) and ERCOT prices (ADM-0013) can be
  admitted; see the [admission register](../admissions/README.md).
- CAISO, PJM, MISO, ISO New England and SPP stay out, each on an exact
  clause in its terms. NYISO stays under review, because its legal notice
  reserves rights and grants no redistribution permission.
- The registry, manifests and gateway must carry per-source notices. A
  release that drops a required notice is a defect.
- The observation spec needs a minor version adding `exchange_referenced`
  before dairy, swine formula and combined COT records are emitted.
- Each admission under this route is rechecked when the publisher's terms
  change, like every other admission.

## Alternatives considered

- **Leave rule 2 unchanged:** this keeps out SOFR and EFFR, which
  short-term rate futures settle against, and ERCOT hub prices, although
  their publishers expressly allow redistribution.
- **Admit any source whose terms are silent:** silence is not a grant, and
  operators that reserve rights could later object. Admissions must rest on
  an express permission or on public-domain status.
- **Relicense notice-bearing data as CC0 without notices:** this would
  breach the publishers' conditions. Carrying the notice costs little.

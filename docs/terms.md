# Terms of use and disclaimer

- Version: 1
- Effective: 2026-11-13

This is the canonical text. The site page
[brand/site/terms.html](../brand/site/terms.html) (served as
`https://bleh.xyz/terms.html`) carries the same words; change both together.

## In short

- The project's own data is CC0 1.0 and its code is Apache-2.0. Use them
  for anything.
- Some values come from publishers that attach a notice. Keep that notice
  with the values whenever you pass them on.
- Everything is provided as is. It may be late, wrong, incomplete or
  unavailable. It is not investment advice.
- bleh.xyz is not affiliated with any source it reads.

## 1. What these terms cover

These terms cover the bleh.xyz website and the data the project publishes
from servers it operates, today `https://data.bleh.xyz/`.

bleh.xyz is an **early preview**. The only data service that is live is
the web snapshot at `https://data.bleh.xyz/web/v0/`, described in
[web snapshot v0](specs/web-snapshot-v0.md). The live gateway, relays and
archive described elsewhere in this repository are not open to the public
yet. These terms will cover them when they are.

Copies served by other people, such as mirrors, are outside the project's
control. These terms still describe the license of the data in them.

## 2. Licenses

- **Data:** the project's own contribution to the data is dedicated to the
  public domain under [CC0 1.0 Universal](../LICENSE-DATA). That includes
  the records as the project formats and computes them, derived values,
  registry entries, method documents and the snapshot files. You may copy,
  change, redistribute and sell it without asking and without credit.
- **Code:** the code is licensed under the
  [Apache License 2.0](../LICENSE).

The licensing decision is [ADR 0002](decisions/0002-licensing.md). CC0
covers only rights the project holds. It does not override a source's own
conditions, which section 3 explains.

## 3. Source notices travel with the data

Every value comes from a source admitted under the
[admission policy](admission-policy.md), and every record names its
admission ID. Some sources let anyone copy and redistribute their data on
conditions: a notice or disclaimer that travels with the data, credit, no
implied endorsement, and labelling of changed values. For those records the
project's CC0 dedication still applies to its own contribution, and the
source's conditions pass through to you
([ADR 0012](decisions/0012-open-terms-route-for-public-bodies-and-market-operators.md),
decision 5).

**Where the notices are.** Every published file carries the notices for
the records in it:

- in the web snapshot, the `notice` field of each entry in `latest.json`
  `sources[]` and of every per-source file;
- in a records archive, `_notices.json` at the archive root and the
  `notices` list of every signed manifest
  ([manifest v0](specs/manifest-v0.md)), once archives are published.

**What you must do.** When you show, republish or redistribute values,
keep each source's notice with the values it covers. Do not suggest that a
source endorses you or your product. Label values you have changed, and do
not present changed values as the source's own.

**The main example** is the Federal Reserve Bank of New York's reference
rates ([ADM-0007](admissions/ADM-0007-nyfed-reference-rates.md)). Their
notice says the rates are subject to the New York Fed's Terms of Use, that
the New York Fed is not responsible for their republication and has no
liability for your use, and that bleh.xyz is not affiliated with or
endorsed by it. For SOFR and BGCR, and rates derived from them, the notice
also carries the disclaimer the New York Fed requires for data it uses
under license from DTCC Solutions LLC. Copy the notice as published; do not
paraphrase it.

Some other notices are a credit line or a condition that values presented
as the publisher's own stay unaltered. The simple rule covers all of them:
keep every notice with its values.

## 4. No warranty, no advice

The data, the site and the code are provided **as is and as available**,
without warranty of any kind, express or implied, including accuracy,
completeness, timeliness, fitness for a particular purpose and
availability. To the extent the law allows, the project and its
contributors are not liable for any loss or damage arising from their use.
The code's own warranty and liability terms are in the Apache License 2.0.

In particular:

- Values can be late, revised by their publisher, missing or wrong. A
  record shows its source time and revision; outages are published as gaps
  and never filled in.
- A value here is a copy, not the publisher's official publication. For
  anything that matters, check the publisher.
- Nothing here is investment, financial, legal or tax advice, or an offer
  or solicitation to buy or sell anything.
- bleh.xyz is an independent project. It is not affiliated with, endorsed
  by or licensed by any source it reads, or by any exchange, venue or data
  vendor. Reading a source does not mean the source approves of bleh.xyz.

## 5. Availability

There is no service level agreement and no guaranteed availability.

- **Today:** the web snapshot is an early preview with no availability
  target. It is normally refreshed every few minutes, and it may be stale,
  change or be unreachable at any time. `latest.json` carries
  `generated_ms` and each source's `last_record_ms`, so you can check
  freshness yourself.
- **Published target, once the live gateway is in production:** at least
  99.9% per 30 days, measured by an external probe that connects once a
  minute, subscribes to one instrument and must receive a block heartbeat
  within 5 seconds, with announced node upgrades excluded and listed on the
  status page ([requirements](requirements.md)). This is a target, not a
  promise. The measured figure will be published as measured.

Field names in a versioned path such as `v0` are not removed or renamed in
place; such a change gets a new path (`v1` beside `v0`,
[ADR 0016](decisions/0016-web-snapshots.md)).

No key or sign-up is needed. Fetch `latest.json` and then only the files
whose `sha256` has changed. Traffic that degrades the service for others
may be limited.

## 6. Reporting problems

No dedicated reporting address is set yet. Until one is, use the
repository on GitHub, [codeurge/bleh.xyz](https://github.com/codeurge/bleh.xyz):

- **Wrong values, broken files, bugs and questions:** open a GitHub issue.
- **Security problems:** vulnerabilities, signing-key compromise, forged
  observations, and **contamination** (any sign that an output contains
  exchange-licensed data or data derived from it). Report these privately
  through GitHub private vulnerability reporting on the repository, never
  in a public issue. [SECURITY.md](../SECURITY.md) describes what happens
  next.
- **A publisher's concerns:** a publisher who believes the project carries
  its data against its terms can report it the same private way. Affected
  outputs are withdrawn under the admission policy's
  [Removal](admission-policy.md#removal) rule.
- **Conduct:** the [code of conduct](../CODE_OF_CONDUCT.md) applies to all
  project spaces. Report unacceptable behavior privately to the
  maintainers; reports are handled confidentially.

When a reporting address is set, it will be published in
[SECURITY.md](../SECURITY.md), the code of conduct and these terms.

## 7. Changes to these terms

These terms may change. Each change gets a new version number and date
here and an entry in [CHANGELOG.md](../CHANGELOG.md); earlier versions
stay in the repository's history. A change cannot take back the CC0
dedication or the Apache-2.0 license of anything already published under
them.

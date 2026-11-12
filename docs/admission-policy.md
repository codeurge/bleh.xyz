# Source admission policy

The mesh's value is independent price discovery: prices formed on open
venues and in reported transactions, not echoes of exchange prices. This
policy decides which fields enter, based on their lineage.

## A source is admissible when all of these hold

1. **Original price formation.** The prices come from trades or quotes made on
   the source itself, or from transactions reported to it, not from
   re-publishing another venue's prices.
2. **No contract on the path.** Getting the data does not require accepting
   terms that restrict redistribution. That means one of:
   - reading public chain state from a node we run,
   - a bucket or archive that the source's operator publishes openly (for
     example a venue's public node-data bucket, even when the storage
     provider bills the requester for transfer),
   - data that law or regulation requires to be published free of charge, or
   - public-domain government data.
3. **Venue-matched fields only.** Published price and size fields come from
   the venue's own matching, never from an exchange feed, an oracle, a
   deployer-supplied price or a commercial vendor. Where an outside price
   affects how a market trades (an oracle-anchored mark band, oracle-based
   funding, liquidations at mark), the record describes the mechanism, and
   every affected record carries the `oracle_anchored_venue` flag. Fills that
   are priced by an oracle or mark rather than matched in the book (for
   example liquidation-at-mark or settlement fills) are excluded.
4. **Field-level review.** Admission is per field, not per source. For
   example, a venue's trades are admitted while its oracle and mark fields
   are excluded.
5. **Recorded decision.** Each admission records the source, the admitted
   fields, how we read them, the reviewer and the date. A maintainer's
   engineering review of field lineage is enough to admit. Admissions are
   reviewed again when the source's methods or terms change.

Rule 2's open-bucket route and rule 3's wording were set on 2026-11-11
(owner decision; see [ADR 0009](decisions/0009-admission-policy-amendments.md)).

## Contributor rules

- Contributors may publish **only their own off-exchange (bilateral or OTC)
  trades and quotes**, signed with their own key.
- Contributors must never relay exchange fills, broker screens or vendor
  feeds.
- Contributions from a key that breaks these rules are removed and the key is
  revoked.

## Naming

- Outputs use neutral, descriptive names that say what the data is: for
  example "WTI front composite" or "US large-cap index front". Exchange
  product names and index brand names are not used as output names.
- Every market carries a liquidity grade. Thin markets are labelled as
  tapes, not benchmarks.
- Registry entries may refer to an exchange contract or a named index as the
  economic reference ("tracks the front WTI futures month"), stated as a fact.

## Removal

If a field turns out to fail this policy, it is removed from live outputs at
once. The archive is versioned, so removal is a new version, not an edit:

- **Filtered raw** (the public, hashed reproducibility root) and
  **normalized partitions** containing the field are rebuilt without it. A
  new manifest version records the change; the old hashes stay listed so the
  change is visible, and the rebuilt files replace the old ones on every
  distribution channel the project operates.
- **Unfiltered raw** node output is private (it contains owner addresses)
  and is never published, so removal does not touch it.

Composite methods tolerate any one source being removed.

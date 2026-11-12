# Source admission policy

The mesh is only valuable if its data is free of exchange data contracts.
This policy decides what may enter. When in doubt, a source stays out.

## A source is admissible only if all of these hold

1. **Original price formation.** The prices come from trades or quotes made on
   the source itself, or from transactions reported to it, not from
   re-publishing another venue's prices.
2. **No contract on the path.** Getting the data does not require accepting
   terms that restrict redistribution. That means one of:
   - reading public chain state from a node we run,
   - data that law or regulation requires to be published free of charge, or
   - public-domain government data.
3. **Clean upstream.** No input to the published fields comes from an exchange
   feed, an oracle fed by exchange data, or a commercial vendor.
4. **Field-level review.** Admission is per field, not per source. For
   example, a venue's trades may be admitted while its oracle and mark fields
   are excluded.
5. **Recorded decision.** Each admission records the source, the admitted
   fields, the legal basis, the reviewer and the date. Admissions are reviewed
   again when the source's terms or methods change.

## Contributor rules

- Contributors may publish **only their own off-exchange (bilateral or OTC)
  trades and quotes**, signed with their own key.
- Contributors must never relay exchange fills, broker screens or vendor
  feeds.
- Contributions from a key that breaks these rules are removed and the key is
  revoked.

## Naming

- Outputs use neutral names (for example "WTI front composite"). Never use
  exchange product names or trademarks as output names.
- Registry entries may refer to an exchange contract as the economic
  reference ("tracks the front WTI futures month"), stated as a fact.

## Removal

If a source turns out to fail this policy, its data is removed from live
outputs at once. Affected archive segments are marked and, where needed,
rebuilt without it. Composite methods must therefore tolerate any one source
being removed.

## What and why

<!-- One concern per pull request. -->

## Checklist

- [ ] No exchange-licensed data, or data derived from it, anywhere in this change (including tests and fixtures)
- [ ] Any new source fields are covered by an `Admitted` record in `docs/admissions/`
- [ ] Decision record added or updated if this changes the data model, wire format, a method, the admission policy, license or governance
- [ ] Owning plan in `.agents/plans/` updated
- [ ] `CHANGELOG.md` updated under Unreleased, if user-visible
- [ ] `scripts/check.sh` passes, and every commit is signed off (`git commit -s`)

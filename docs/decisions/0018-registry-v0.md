# 0018: Registry document, v0

- Status: Accepted
- Date: 2026-11-13
- Deciders: Repository owner

## Context

[architecture.md](../architecture.md) § 10 describes the instrument
registry as instruments, exposures, venue mappings, contract specs, roll
schedules, session calendars and liquidity grades. The code had three
partial pieces with no common format:

- `mesh_types::Registry`, `{"instruments": [...]}`: venue markets and their
  scales, written by Hyperliquid auto-registration and read by every
  service as `--registry`.
- The reference pollers' catalogue: series families with notices and
  method periods, printed as a bare list.
- The synthetic-months configuration: neutral exposures, calendars, expiry
  and roll rules, private to that method.

Neutral output names, liquidity labels and roll schedules had nowhere to
live that consumers could read, and nothing described the registry
document a registry pull request changes.

## Decision

1. **One registry document, `registry-v0`**
   ([registry-v0.md](../specs/registry-v0.md), with a JSON Schema): the
   existing `instruments` plus optional sections `exposures` (with venue
   mappings), `calendars`, `rolls`, `series` and `liquidity`, and an
   optional version tag `registry`.
2. **Backward compatible for instruments.** `InstrumentSpec` is unchanged.
   A document with only instruments is the old file and is still written
   in the old form, so existing registries, fixtures and services keep
   working. Readers built before this change reject documents with the new
   sections, as unknown fields always are.
3. **Neutral names live on exposures.** Instruments keep the venue's own
   symbol as a mapping key; exposures carry the neutral `id` and `name`,
   and the economic reference only as a fact in `tracks`. Automatic
   registration fills instruments only and never names anything.
4. **Calendars and rolls reuse the synthetic-months vocabulary** (holiday
   rule sets, `ExpiryRule`, fixed-days `RollRule`), so a rule means the
   same dates in every component. Rolls are fixed-days only; anything that
   would need exchange data is left out and listed in the spec (§ 9).
5. **Liquidity grades are snapshots of derived records.** The registry
   copies the latest `liquidity_grade` (or composite grade) per subject;
   a subject without one is a `tape`.
6. **The catalogue is the `series` section.** Its output validates against
   the schema unchanged.

## Consequences

- Services that take `--registry` accept the full document; operators can
  keep exposures, calendars and grades in the same reviewed file the node
  reader updates.
- `mesh_types::Registry` validates references on load (calendars, rolls,
  liquidity subjects). A bad entry fails start-up instead of being
  ignored.
- Tests validate code output against the schema, so the two cannot drift
  silently. They use `boon`, the test-only validator the observation-v0
  schema tests already use.
- Follow-ups: serve exposures and grades from the history API's
  `/v0/instruments`; have a grade publisher refresh the `liquidity`
  section; let the synthetic-months method read exposures from the
  registry in a later method version.

## Alternatives considered

- **Separate files per section.** Rejected: services would need several
  flags, and cross-references (exposure to roll to calendar) could not be
  checked in one load.
- **Names on instruments.** Rejected: auto-registered markets would get
  names derived from venue symbols, which can resemble exchange tickers.
  Keeping names on reviewed exposures keeps the naming rule enforceable.
- **Measured grades only in the stream.** Rejected: the admission policy
  requires every market to carry a grade, and consumers of the registry
  alone could not tell a benchmark from a tape.

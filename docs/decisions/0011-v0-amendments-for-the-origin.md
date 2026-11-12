# 0011: Data model and gateway amendments for the integrated origin

- Status: Accepted
- Date: 2026-11-12
- Deciders: Repository owner (integrator)

## Context

Integrating the build lanes into one origin service
([ADR 0007](0007-code-layout.md), amendment of 2026-11-12) surfaced
requests that change the data model and the wire format, which
[AGENTS.md](../../AGENTS.md) rule 4 requires to be recorded:

- the book builder emitted analytics with no observation kind, so they
  could not be signed, archived or served;
- derived records carried the `seq` of their last input, which lies in a
  block that is already closed (and signed) when they are complete;
- quiet markets and recording gaps looked alike in the archive;
- order-level replay output had no flag of its own;
- swap repositories publish lifecycle actions, event types, option terms,
  monetary spreads and commodity quantities that the model could not hold;
- `#` in partition paths breaks URI-based readers;
- the gateway's heartbeat had no venue, channel caps answered
  `bad_request`, and `l2` subscribers received L4 events they could not
  use.

`observation-v0` and `gateway-v0` are drafts (no public release yet), so
these are amendments of v0 rather than new major versions.

## Decision

Observation v0 (`docs/specs/observation-v0.md`):

1. New derived kinds `activity_bar`, `delta_bar`, `profile_summary`,
   `order_activity`, `queue_positions`, `sweep`, `heatmap_tile` and
   `liquidity_grade` (§ 2.10), with a signed fixed-point decimal for delta
   and CVD; field names avoid excluded venue names (`volume_delta`,
   `option_premium`).
2. A non-derived `coverage` kind (§ 2.11) on the venue-wide
   pseudo-instrument `<venue>:venue:all`.
3. A derived record is positioned at, published in and signed with the
   block that completed it: `seq = (B, i)` per instrument and kind
   (§ 2.8). `ts_source_ms` stays the time of its last input. End-of-block
   records keep the end-of-block marker.
4. Flag bit 6 `replayed` (§ 2.9).
5. `SwapPrint` gains actions `modify`, `terminate`, `revive`,
   `event_type`, `strike`, `option_premium`, `option_type`,
   `option_style`, `first_exercise_date`, `quantity` and the
   `spread_amount` price type (§ 2.7).
6. Partition paths write `#` as `%23` (§ 4,
   [archive-layout-v0](../specs/archive-layout-v0.md) § 3.1).
7. Analytics Arrow columns declare their storage in `bleh.encoding`
   metadata (§ 9).

Canonical encodings of existing kinds are unchanged; the shared fast
encoder (`mesh_types::canonical`) is byte-identical to the generic path.

Gateway v0 (`docs/protocol/gateway-v0.md`): the `block` heartbeat carries
`venue`; channel caps answer `limit_exceeded`; `l2` subscribers get
gateway-derived `levels` messages (not covered by commitments); wildcard
channels, every `hello.limits` field, snapshot reasons, the two-strike
slow-consumer policy, close codes and HTTP endpoints are documented as
implemented. Servers and clients upgrade together (unknown fields are
rejected).

## Consequences

- Every output of the book builder can be signed, archived, served and
  verified like trades and book events, and the archive replay reproduces
  the live derived records exactly (checked end to end on synthetic data).
- Consumers can tell quiet hours from unrecorded ones.
- The swap pollers emit records they used to skip; volume consumers must
  select `event_type` `trade` (or absent).
- Readers of `#` instruments must unescape `%23`; URL builders escape `%`.
- Any client of the earlier v0 draft must update for the heartbeat
  `venue`.

## Alternatives considered

- **Keep analytics as unsigned JSON beside the stream**: no signatures, no
  archive, two code paths.
- **Keep the last input's `seq` and sign derived records under a later
  block**: breaks the rule that a record's block is its commitment's
  block, and inclusion proofs keyed by `seq`.
- **Per-market coverage records**: hundreds of identical records per
  interval; coverage is a property of the venue's blocks.
- **A new major version**: unnecessary before the first public release.

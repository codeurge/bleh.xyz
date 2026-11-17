# 0026: The L2 anchor is optional until an L2 is chosen

- Status: Accepted
- Date: 2026-11-16
- Deciders: Repository owner
- Amends: [manifest-v0](../specs/manifest-v0.md) § 6.3 (confirmation
  window)

## Context

manifest-v0 § 6.3 made both anchors required once a manifest is older than
its confirmation window: 48 hours for OpenTimestamps and 24 hours for the
Ethereum L2. But no L2 has been chosen yet (§ 6.2: "the chain is to be
decided"). The records pipeline runs with `ARCHIVE_EVM_CHAIN_ID` empty, so
no manifest has an L2 anchor.

So every published day failed verification 24 hours after signing. That
applied to anyone running `archive verify`, as well as to the pipeline's
own check. A day verified straight after signing passed with a warning,
but a signed day whose publish was retried the next day always failed.
On 2026-11-16 this stalled two archives: `reference-normalized` at
2026-10-30 and `hyperliquid-backfill` at 2025-10-17. Each day was signed,
published and Bitcoin-attested, and failed only on the missing L2 anchor.

## Decision

1. Until an L2 is chosen, a **missing** L2 anchor is a warning at any
   manifest age. This holds whether `ethereum_l2` is absent from
   `manifest.anchors.json` or the anchors file itself is missing.
2. An L2 anchor that **is** present follows § 6.3 unchanged. It is a
   warning while pending inside the 24-hour window, an error if still
   pending after it, and an error whenever it is marked `confirmed` but
   does not verify.
3. The Bitcoin (OpenTimestamps) rules are unchanged, so a day still needs a
   confirmed Bitcoin anchor after 48 hours.
4. Choosing an L2 needs its own decision record. That record says from which
   manifest sequence or date the L2 anchor becomes required again.
   Manifests signed before that point stay valid without one.

## Consequences

- Days signed and published since 2026-11-13, and those held up by the
  stall, verify again, with a warning for the L2 anchor. The two stalled
  archives resume from their stalled days without re-signing anything.
- A verifier that wants the old behaviour can run with `--strict`, which
  treats every warning as a failure.
- Signed manifests, anchors files and the wire format don't change.

## Alternatives considered

- **Exempt only the pipeline's own check.** Every day would still fail for
  outside verifiers 24 hours after signing, which says something false
  about the archive.
- **Hold publishing until an L2 is chosen.** This stops every records
  archive for a decision that hasn't been made, for an anchor the archive
  doesn't use yet.

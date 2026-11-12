# 0014: User-supplied data stays on the user's machine

- Status: Proposed
- Date: 2026-11-12
- Deciders: Repository owner

## Context

The open reference prices plan
([.agents/plans/open-reference-prices.md](../../.agents/plans/open-reference-prices.md))
aims to get as close as clean lineage allows to a full alternative for
exchange futures data. Some users already license an exchange feed
themselves, from a market-data vendor or as a broker export. They want
bleh.xyz prices next to that feed, and they want to measure how far the
open prices are from what they pay for.

If the licensed feed has to be the product, whoever licenses it holds the
leverage. If it is an optional plug-in on the user's own machine, it does
not. That only works if the plug-in can never move licensed data into the
project. [AGENTS.md](../../AGENTS.md) hard rule 1 forbids exchange data in
the repository, the archive or the network, including small samples and
test fixtures. The data license ([ADR 0002](0002-licensing.md)) releases
everything the project publishes under CC0, which is impossible for data a
user licenses from someone else. A server that accepted user feeds, or a
client that could sign, publish or archive them, would break both rules and
make the project a redistributor of licensed data.

## Decision

1. **Boundary.** Data from a feed the user licenses themselves
   (*user-supplied data*) is processed only by the client, on the user's
   machine, under the user's own license. It is:
   - never sent to any bleh.xyz service (gateway, historical API, origin,
     archive, relays, mirrors or telemetry);
   - never signed with project keys, and never passed to the project's
     canonical encoding, Merkle or signing code;
   - never written into an archive layout, a manifest or a publishing path.
2. **Tag.** Every output built from user-supplied data (merged lines,
   tracking statistics, local websocket frames) carries
   `"provenance":"user_supplied"`. The tag is local to `tools/mesh-client`.
   It is not a `mesh-types` flag or record kind, because user-supplied data
   never becomes a mesh record.
3. **Connectors, never data.** The project ships connectors only: code that
   reads documented open formats and APIs with the user's own credentials.
   The first connectors read DBN files (the open Databento Binary Encoding,
   through the Apache-2.0 `dbn` crate), the vendor's live API through its
   Apache-2.0 client (behind the off-by-default `databento-live` feature,
   with the API key read from an environment variable the user sets) and
   CSV or JSON lines with configurable columns. Connectors are clean-room
   implementations from published specifications and public library APIs
   (AGENTS.md rule 6). The repository holds no user data, no vendor samples,
   no credentials and no vendor dataset codes in code.
4. **Synthetic tests.** Connector and merge tests use data generated in the
   test (DBN records written with the `dbn` encoder in memory, inline CSV),
   or synthetic files under `fixtures/synthetic/`.
5. **Enforced in code**, in `mesh_client::byo`:
   - user-supplied values live in `UserSupplied<T>`, which implements no
     serde traits and no conversion into `mesh_types` records. Compile-fail
     doctests prove that it cannot reach the canonical encoder, a Merkle
     leaf, `serde_json`, or a `mesh_types::Record`;
   - output files are named only through `LocalPath`, which refuses a path
     inside a bleh.xyz archive (an ancestor holding `_manifests/`,
     `_iceberg/` or a signed bleh manifest, or a Hive partition directory)
     or inside a bleh.xyz source checkout;
   - the local websocket binds only a literal loopback address
     (`LoopbackAddr`), and refuses browser connections from origins other
     than loopback pages unless the user allows one explicitly;
   - the gateway connection is read-only: the merge never calls
     `Client::send`, and the client's protocol messages have no field that
     could carry user data;
   - a source-scan test fails if the `byo` module references signing,
     Merkle, manifest, hashing, key, archive or bus code.
6. **Derived outputs stay the user's.** Statistics computed from
   user-supplied data (basis, tracking error, lead/lag) are user-supplied
   too. The project does not ask for them and does not ingest them. They are
   not contributions under the contributor rules in
   [docs/admission-policy.md](../admission-policy.md), which admit only a
   contributor's own off-exchange trades and quotes.
7. **The user is responsible for their license.** bleh.xyz grants no rights
   to the user's feed and checks none. The user guide says so.

## Consequences

- A user who licenses a feed gets an exchange-equivalent view locally:
  bleh.xyz prices and their own feed side by side, with tracking statistics
  that measure the open prices against what they license, and nothing
  shared.
- The licensed feed becomes an optional local plug-in, not the product.
- The project cannot learn from users' comparisons. If users publish
  aggregate results themselves, that is their decision under their license,
  and the project still does not ingest them as data.
- Every new output path in the adapter must go through `LocalPath` or a
  loopback-only listener. Widening the boundary (for example, any upload,
  sharing or telemetry feature) needs a new decision record.
- The live connector adds an optional dependency tree; it passes
  `cargo deny` and stays off unless a user enables the feature.

## Alternatives considered

- **Server-side merge** (users upload their feed to a bleh.xyz service):
  the project would receive and process licensed data, breaking hard
  rule 1 and making it a redistributor.
- **A `user_supplied` flag or record kind in `mesh-types`**: a mesh record
  can be encoded, signed, published and archived, so the type would open
  exactly the path this decision closes.
- **Documentation only, no connectors**: leaves the work, and so the
  leverage, with whoever licenses the feed.
- **Vendor sample files as test fixtures**: forbidden by hard rule 1, even
  when small.
- **A websocket on any interface**: one flag away from redistributing the
  feed on a network.

# Daily archive manifest, v0

- Status: Draft, types implemented in `crates/mesh-types` (`manifest`
  module); builder and verifier tool in `crates/archive`
- Format tag: `bleh-manifest-v0`

A manifest lists every file of one archive day, signed, hash-chained to the
previous day, and anchored on Bitcoin and an Ethereum L2. Archive days are
distributed over HTTPS, BitTorrent v2 (with web seeds) and IPFS from day
one. The manifest ties the three together: whichever channel a file comes
from, the manifest says what its bytes must be.

## 1. Layout of a day

Each archive (for example `hyperliquid-raw` or `hyperliquid-normalized`;
see [archive-layout-v0.md](archive-layout-v0.md)) keeps its data in a
Hive-style tree under the archive root. A day's files are those whose
`date=` partition is that day. Manifests sit beside the data:

```text
<archive>/kind=.../date=<YYYY-MM-DD>/...     data files (paths listed in the manifest)
<archive>/_manifests/<YYYY-MM-DD>/
  manifest.json            signed manifest (§ 2-3)
  manifest.anchors.json    anchors (§ 6), not signed, updated as anchors confirm
  manifest.json.ots        OpenTimestamps proof (§ 6.1)
```

The data files are the day's torrent content and the day's IPFS content.
`manifest.json` cannot be inside the torrent (it contains the infohash),
so it is published over HTTPS and IPFS beside it.

## 2. Manifest body

`manifest.json` is a `SignedManifest`: `{"manifest": {...}, "signature":
"0x..."}`. The body:

| Field | Type | Meaning |
|---|---|---|
| `format` | string | `bleh-manifest-v0` |
| `archive` | slug | Archive name, `[a-z0-9][a-z0-9-]{0,31}` |
| `date` | `YYYY-MM-DD` | UTC day |
| `sequence` | integer | Position in this archive's chain; 0 for the first manifest |
| `created_ns` | ns string | Build time |
| `previous_manifest_sha256` | hex or null | Manifest hash (§ 4) of the previous day; null only at sequence 0 |
| `files` | array | § 2.1, sorted by `path`, unique |
| `torrent` | object | § 2.2 |
| `web_seeds` | array of URLs | HTTPS base URLs, each ending in `/` (§ 5.1) |
| `signer` | address | Ethereum address of the signing key |
| `next_signer` | address, optional | Key that may sign the next manifest instead (§ 3.2) |
| `notices` | array, optional | Source notices (§ 2.3); omitted when empty |

A day with no files has no manifest. A gap in dates is allowed; a gap in
`sequence` is not.

### 2.1 Files

| Field | Type | Meaning |
|---|---|---|
| `path` | string | Relative to the archive root, `/`-separated, printable ASCII, no `..`, `.`, empty segments, leading `/` or `\` |
| `size` | integer | Bytes |
| `sha256` | hex | SHA-256 of the bytes. **The authoritative integrity check** |
| `content_hash` | hex, optional | Canonical content hash of the records ([observation-v0](observation-v0.md) § 6.2); normalized partitions only |
| `record_count` | integer, optional | Records in the file; present exactly when `content_hash` is |
| `ipfs.cid` | string | CIDv1, multibase base32 (`bafk...` raw, `bafy...` dag-pb), multihash sha2-256 |
| `ipfs.codec` | `raw` or `dag-pb` | `raw` if the file fits one chunk, otherwise a UnixFS `dag-pb` DAG |
| `ipfs.chunker` | string | `size-1048576` in v0 |
| `ipfs.raw_leaves` | boolean | `true` in v0 |
| `bt_pieces_root` | hex, optional | The file's BitTorrent v2 `pieces root` (SHA-256 Merkle root over 16 KiB blocks, BEP 52) |

The IPFS parameters reproduce the CID with
`ipfs add --cid-version=1 --raw-leaves --chunker=size-1048576` (balanced
DAG layout, the default). They are recorded per file so a later change of
parameters stays verifiable.

### 2.2 Torrent

| Field | Meaning |
|---|---|
| `infohash_v2` | SHA-256 of the bencoded `info` dictionary (BEP 52), 64 hex digits |
| `magnet` | `magnet:?xt=urn:btmh:1220<infohash_v2>&dn=<archive>-<YYYY-MM-DD>&ws=<web seed>...` |

The torrent is v2-only, named `<archive>`, with a piece length of 1 MiB
(2^20), containing exactly the manifest's files at their paths. Every
day's torrent has the same name and root, so a client that seeds several
days shares one directory tree. With
these fixed parameters anyone can rebuild the torrent from the files and
get the same infohash.

### 2.3 Source notices

Sources admitted under the open-terms route (admission policy rule 2,
[ADR 0012](../decisions/0012-open-terms-route-for-public-bodies-and-market-operators.md),
decision 5) may require a notice to travel with their data, for example
the New York Fed's reference-rates notice. The manifest carries them, so
the notice is signed and distributed with every day of the archive.

| Field | Type | Meaning |
|---|---|---|
| `admission` | AdmissionId | Admission of the records the notice covers |
| `source` | VenueId | The records' `prov.venue` |
| `notice` | string | The notice text as the admission record requires it, 1 to 4096 bytes, no control characters |

Entries are sorted by `admission`, then `source`, and unique. The list is
omitted when empty, so manifests of archives with no such source keep
their encoding. The archive builder reads the list from `_notices.json` at
the archive root (a JSON array of these objects), which the pollers that
fill the archive write; files whose path has a segment starting with `_`
are never data files.

## 3. Signature

### 3.1 Scheme

The mesh uses secp256k1 for every signature (block commitments and
manifests), Ethereum style, so contracts can verify them with `ecrecover`.
This replaces the ed25519 signature first proposed for manifests: one key
type and one verifier for everything.

```text
body  = canonical_json(manifest)            observation-v0 § 6.1 rules
h     = keccak256("bleh-manifest-v0\n" || body)
digest = keccak256("\x19Ethereum Signed Message:\n32" || h)   EIP-191
signature = secp256k1_sign(digest)          65 bytes r || s || v, low s, v in {27, 28}
```

`signature` is `0x` and 130 lowercase hex digits. A verifier recovers the
address from `signature` and `digest` and compares it with `signer`.

### 3.2 Keys and rotation

- Signer addresses are published in the registry with validity dates and
  revocations. A verifier trusts a manifest only if its `signer` is a
  registered, unrevoked key.
- Rotation: a manifest signed by the current key names `next_signer`. The
  following manifest may be signed by either key. Once a manifest is signed
  by the new key, the old key is retired for that archive.
- An unannounced signer change breaks the chain (§ 4) unless the registry
  records an emergency rotation, which needs maintainer review.

## 4. Manifest hash and chain

```text
manifest_hash = SHA-256( canonical_json(signed_manifest) )
```

computed over the whole `SignedManifest`, signature included. The next
manifest's `previous_manifest_sha256` is this hash, and the anchors commit
to it. SHA-256 is used here (not Keccak) because OpenTimestamps and the
distribution channels are SHA-256 based.

A link from `prev` to `next` is valid when: same `archive`;
`next.sequence = prev.sequence + 1`; `next.date > prev.date`;
`next.previous_manifest_sha256 = manifest_hash(prev)`; and `next.signer` is
`prev.signer` or `prev.next_signer`.

## 5. Distribution

### 5.1 HTTPS

A file is at `<web seed><archive>/<path>`, which is also the BEP 19
web-seed URL rule for a multi-file torrent named `<archive>`. Every web
seed serves the same bytes; mirrors add themselves to future manifests.

### 5.2 BitTorrent v2

From the `magnet` link or a `.torrent` rebuilt from the files (§ 2.2).
Clients use the web seeds when no peers are available.

### 5.3 IPFS

By `ipfs.cid`. Mirrors pin the day's CIDs and the manifest's own CID.

## 6. Anchors

Anchors are created after signing, so they live beside the signed manifest
in `manifest.anchors.json` (`ManifestAnchors`), never inside it. Each
anchor proves itself; the file is not signed.

| Field | Meaning |
|---|---|
| `format` | `bleh-anchors-v0` |
| `manifest_sha256` | The anchored manifest hash (§ 4) |
| `opentimestamps` | § 6.1, optional |
| `ethereum_l2` | § 6.2, optional |

### 6.1 Bitcoin (OpenTimestamps)

| Field | Meaning |
|---|---|
| `proof_path` | `manifest.json.ots` |
| `proof_sha256` | SHA-256 of the `.ots` file |
| `status` | `pending` (calendar promise only) or `confirmed` (upgraded to a Bitcoin attestation) |
| `bitcoin_block_height` | Attested block, once confirmed |

The timestamped digest is `manifest_hash`, the SHA-256 of the canonical
signed manifest bytes. The proof is upgraded in place once a Bitcoin block
confirms it, and `status` becomes `confirmed`.

### 6.2 Ethereum L2

| Field | Meaning |
|---|---|
| `chain_id` | EIP-155 chain ID (the chain is to be decided) |
| `tx_hash` | Transaction that records the hash |
| `block_number` | Block including it |
| `committed_hash` | The 32 bytes committed on chain; must equal `manifest_hash` |
| `status` | `pending` until the transaction is final on the L2, then `confirmed` |

The transaction calls an anchoring contract (to be specified with the
chain choice) that emits `committed_hash`, or carries it as calldata.

### 6.3 Confirmation window

A missing or pending anchor is a **warning** while the manifest is younger
than the confirmation window: 48 hours for OpenTimestamps, 24 hours for the
L2. After the window, the OpenTimestamps anchor is **required** and must be
`confirmed`, and an L2 anchor that is present must be `confirmed`. Until an
L2 is chosen, a **missing** L2 anchor stays a warning at any age
([ADR 0026](../decisions/0026-l2-anchor-optional-until-chosen.md)). An
anchor marked `confirmed` whose proof does not verify is always an error.

## 7. Verification

Order: **signature, then hash chain, then anchors, then files.**

1. **Signature.** Parse `manifest.json`; check the structure rules (§ 2;
   `Manifest::validate`); check `signer` is a trusted key (§ 3.2); recover
   the signer from the signature (§ 3.1) and compare.
2. **Hash chain.** Walk back with `previous_manifest_sha256` to a manifest
   the verifier already trusts (a checkpoint, or sequence 0), checking each
   link (§ 4) and each manifest's signature.
3. **Anchors.** Check `manifest.anchors.json` refers to this manifest and
   is consistent (`ManifestAnchors::check`), then verify the OpenTimestamps
   proof against Bitcoin and the L2 transaction against the chain. Apply
   § 6.3 for missing or pending anchors.
4. **Files.** For each downloaded file: size equals `size`; SHA-256 equals
   `sha256`.

A file that passes step 4 under a manifest that passed steps 1-3 is
genuine, whatever channel or mirror it came from.

### 7.1 Mirror checklist

A mirror verifies a downloaded file against the signed manifest like this:

1. Fetch `manifest.json` (any channel) and run steps 1-3.
2. Fetch the file (HTTPS, torrent or IPFS).
3. Check `size` and `sha256` (step 4). Reject on mismatch and fetch again
   elsewhere.
4. Optionally recompute `ipfs.cid` with the recorded parameters and
   `bt_pieces_root`, and confirm they match before serving the file on
   those channels.
5. Optionally, for a normalized partition, read the records and recompute
   `content_hash` and `record_count` (this also checks the writer is
   deterministic).
6. Serve the file only after step 3, and pin or seed it under the CID and
   infohash in the manifest.

`archive verify` (in `crates/archive`) implements steps 1-4 and the mirror
checklist.

## 8. Versioning

`bleh-manifest-v0` is a draft. Any change to fields or canonical encoding
is a new format tag. Verifiers reject unknown formats and unknown fields.

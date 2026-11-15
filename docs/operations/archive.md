# Archive operations

- Owner: archive lane (`crates/archive`)
- Specs: [archive-layout-v0](../specs/archive-layout-v0.md),
  [manifest-v0](../specs/manifest-v0.md),
  [observation-v0](../specs/observation-v0.md)
- Decisions: [ADR 0006](../decisions/0006-phase-1-2-service-architecture.md),
  [ADR 0007](../decisions/0007-code-layout.md)

This runbook covers the `archive` binary: raw capture, partitions, replay,
manifests, anchors, publishing and verification, and how anyone can mirror
and verify the archive.

## 1. Archives and layout

| Archive | Tier | Contents | Published |
|---|---|---|---|
| `hyperliquid-raw-private` | private | Node lines as written (owner addresses and other excluded fields included) | **Never** |
| `hyperliquid-raw` | public root | Node lines with excluded streams and fields removed by the node reader's filter | Yes |
| `hyperliquid-normalized` | public | Parquet partitions rebuilt from `hyperliquid-raw` by replay | Yes |

```text
hyperliquid-raw-private/stream=<stream>/date=<YYYY-MM-DD>/<HH>.jsonl.zst
hyperliquid-raw-private/_dictionaries/<stream>/<dict-id>.zdict
hyperliquid-raw/stream=<stream>/date=<YYYY-MM-DD>/<HH>.jsonl.zst
hyperliquid-normalized/kind=<kind>/venue=<venue>/instrument=<id with ~>/date=<YYYY-MM-DD>/part-<NNNNN>.parquet
hyperliquid-normalized/_iceberg/<kind>/metadata/{v<N>.metadata.json,version-hint.text,*.avro}
<archive>/_manifests/<YYYY-MM-DD>/manifest.json            signed (canonical JSON; SHA-256 = manifest hash)
<archive>/_manifests/<YYYY-MM-DD>/manifest.anchors.json    anchors (mutable)
<archive>/_manifests/<YYYY-MM-DD>/manifest.json.ots        OpenTimestamps proof (mutable)
<archive>/_manifests/<YYYY-MM-DD>/<archive>-<YYYY-MM-DD>.torrent
<archive>/_manifests/index.json                            every manifest's date, sequence, hash (mutable, untrusted)
<archive>/_manifests/magnets.txt                           every day's magnet (mutable)
```

Pinned settings (the reproducibility contract):

| Item | Setting |
|---|---|
| Filtered raw | zstd level 19, one frame, no dictionary, content size and checksum flags, single-threaded |
| Private raw | zstd level 19 (configurable), long-distance matching with a 2^27 window, optional per-stream dictionary |
| Parquet | zstd level 9; row groups of 131,072 rows; parts of 16,777,216 rows; 1 MiB v1 data pages; dictionary only on `Utf8` columns; page statistics and page index; no bloom filters; `created_by = bleh-archive`; key-value metadata exactly `bleh.schema`, `bleh.kind`, `bleh.content_hash`, `bleh.record_count`; no Arrow IPC schema; Parquet field IDs for Iceberg |
| IPFS | CIDv1, raw leaves, `size-1048576` chunks, balanced layout, 174 links per node (`ipfs add --cid-version=1 --raw-leaves --chunker=size-1048576`) |
| Torrent | v2 only, name = archive, 1 MiB pieces, 16 KiB Merkle blocks, web seeds in `url-list`, no creation date |

`bleh.content_hash` is the gate: anyone replaying the raw archive must get
records with the same canonical content hash. Byte-identical Parquet is
guaranteed only for the same `archive` crate and Parquet library versions
(the pinned build container).

## 2. Daily pipeline

```text
node files ──capture──▶ private raw ──upload──▶ private bucket
             │
             └─filter──▶ filtered raw ──upload──▶ public bucket ─┐
                              │                                  │
                              └──replay──▶ partitions ───────────┤
                                                                 ▼
                     manifest build/sign ─▶ anchor (OTS, L2) ─▶ publish (S3/R2, IPFS, torrent) ─▶ verify
```

For day `D`, after hour `00` of `D + 1` is sealed (replay reads it for
blocks just before midnight):

```bash
export BLEH_ARCHIVE_SIGNING_KEY_FILE=/etc/bleh/manifest.key
R=/data/archive

# 1. Raw tiers. Live, the origin captures them (docs/operations/origin.md);
#    standalone: `archive capture run --follow`; batch (catching up):
archive capture run --staging /data/staging \
  --stream fills=/hl/data/node_fills_by_block \
  --stream order_statuses=/hl/data/node_order_statuses_by_block \
  --stream raw_book_diffs=/hl/data/node_raw_book_diffs_by_block \
  --upload

# 2. Partitions from filtered raw (the only path to partitions). The parser
#    needs the reader's registry; BLEH_BOOK_CHECKPOINT_SECS must match the
#    origin's (default 600).
export BLEH_ARCHIVE_REGISTRY=/data/registry.json
archive replay --raw $R/hyperliquid-raw --date $D --out $R/hyperliquid-normalized

# 3. Manifests, one chain per archive.
for A in hyperliquid-raw hyperliquid-normalized; do
  archive manifest build --root $R/$A --archive $A --date $D \
    --web-seed https://data.example.org/ --web-seed https://mirror.example.net/ --sign
done

# 4. Anchors.
archive anchor ots-stamp   --root $R/hyperliquid-normalized --date $D
archive anchor evm-submit  --root $R/hyperliquid-normalized --date $D --chain-id <id> \
  --rpc https://rpc.example --key-file /etc/bleh/anchor.key
# later (hourly cron) until confirmed:
archive anchor ots-upgrade --root $R/hyperliquid-normalized --date $D --bitcoin-api https://blockstream.info/api
archive anchor evm-poll    --root $R/hyperliquid-normalized --date $D --chain-id <id> \
  --rpc https://rpc.example --key-file /etc/bleh/anchor.key

# 5. Publish (and re-publish after each anchor update; uploads are idempotent).
archive publish --root $R/hyperliquid-normalized --archive hyperliquid-normalized --date $D \
  --upload --kubo http://127.0.0.1:5001 --iceberg-location s3://bucket/hyperliquid-normalized

# 6. Verify what was published, as a mirror would.
archive verify --url https://data.example.org/hyperliquid-normalized --archive hyperliquid-normalized \
  --date $D --trusted 0x<signer> --deep --bitcoin-api https://blockstream.info/api --evm-rpc https://rpc.example
```

All commands take `--log-format json` for structured logs and
`--metrics-out FILE` to write Prometheus counters (textfile collector):
`archive_capture_lines_total`, `archive_capture_bytes_total`,
`archive_capture_files_sealed_total`, `archive_capture_filtered_out_total`,
`archive_capture_stream_excluded_total`, `archive_store_put_*_total`,
`archive_store_bytes_uploaded_total`, `archive_replay_records_total`,
`archive_ots_submissions_total`, `archive_evm_anchor_submitted_total`,
`archive_publish_days_total`.

## 3. Commands

| Command | What it does |
|---|---|
| `capture run` | Feeds node hour files (`<dir>/hourly/<YYYYMMDD>/<H>`) to both tiers, seals every hour, optionally uploads. Resumes open hours after a restart (skips the lines already captured). With `--follow` it keeps running (see "Live capture") |
| `capture upload` | Uploads sealed hours in `<staging>/<archive>/ready/` write-once; conflicts go to `conflicts/` and fail the command |
| `capture train-dict` | Trains a private-tier zstd dictionary from node files (`--dict stream=FILE` uses it) |
| `partition` | Writes partitions from records in JSON lines (for derived records produced elsewhere) |
| `partition-dir` | Partitions a directory of daily record files (`date=YYYY-MM-DD/<file>`, as swap-pollers writes) once per settled day; `--follow` keeps scanning (see "Daily record files") |
| `replay` | Rebuilds a day from filtered raw; `--check-against DIR [--bytes]` compares with existing partitions |
| `iceberg` | Rebuilds static Iceberg metadata from every manifest for a given location |
| `manifest build` / `sign` / `verify` / `keygen` | Builds the day's manifest and torrent, signs, verifies (no file downloads), generates a key |
| `anchor ots-stamp` / `ots-upgrade` | Submits the manifest hash to calendars; upgrades to a Bitcoin attestation (checked against headers if `--bitcoin-api`) |
| `anchor evm-submit` / `evm-poll` | Submits (or `--dry-run` signs only) the L2 anchor transaction; records it once included and final |
| `publish` | Checks local files against the manifest, uploads write-once, replaces mutable files, adds and pins on IPFS (CIDs must match), rebuilds Iceberg metadata |
| `verify` | Signature, chain, anchors, then files (`--deep` adds CIDs, pieces roots and partition content hashes); `--json`, `--strict` |

Exit codes: `0` success (warnings allowed unless `--strict`), `1` failure,
`2` usage error; `partition-dir` exits `3` when a day's file changed after
it was partitioned.

### Daily record files

`archive partition-dir --records-dir DIR --file NAME [--file NAME2] --out ROOT
[--state-dir S] [--settle-days 2] [--follow --interval-secs 900]`
(every flag also reads a `BLEH_PARTITION_DIR_*` variable;
`BLEH_PARTITION_DIR_FILE` takes a comma-separated list). Compose runs it as
`swap-partition` for swap prints and `reference-partition` for reference
records (`reference_value.jsonl`, `positioning.jsonl`, settled 12 days to
cover the New York Fed's revision window) on the home-archive server.

- A day is written once it is `--settle-days` whole days before today
  (UTC), when its file is complete (swap-pollers finishes a day from the
  repository's end-of-day file).
- Each written day file leaves a marker, `<state>/<file>/<day>.sha256`,
  holding its SHA-256; unchanged files are skipped on later scans.
- Memory does not grow with the day file. One pass checks every record
  (validity, allowlist, duplicate `seq`) and spills each line to a bucket
  file under `<state>/<file>/<day>.spill/`, grouped by partition; the
  buckets are then written one at a time (about 64 MiB of records each,
  or one whole partition if larger) and the spill directory is removed.
  The state directory needs free space about the size of the largest day
  file. The partitions are the same files, byte for byte, as partitioning
  the whole day in memory.
- Partitions are write-once. A file that changes after its day was
  partitioned is logged and counted
  (`archive_partition_dir_changed_total`) and left alone; correct it
  deliberately with `archive partition --correction`.
- On a self-hosted server without object storage, set
  `LOCAL_NORMALIZED_DIR` to a host directory and `HISTORY_API_ARCHIVE` to
  `/srv/normalized` to serve the result locally.

### Records archives

The partitioners write two archives in place
([ADR 0017](../decisions/0017-records-archives.md)):
`$LOCAL_NORMALIZED_DIR/reference-normalized` and
`$LOCAL_NORMALIZED_DIR/swaps-normalized`. Serve them with
`HISTORY_API_ARCHIVE=/srv/normalized` and
`HISTORY_API_ARCHIVE_NAMES=reference-normalized,swaps-normalized`.

`deploy/scripts/archive-pipeline.sh records ARCHIVE` signs every day after
the archive's last manifest that is at least `RECORDS_LAG_DAYS` (14) days
old, oldest first: manifest (signed), OpenTimestamps, publish (upload,
Iceberg metadata, IPFS pinning through the `ipfs` service) and, with
`ARCHIVE_TRUSTED_SIGNER`, verification over HTTPS. Run it daily, and
`records-anchors ARCHIVE` a few times a day until the Bitcoin attestations
arrive. A day signed but not published (a failed run) is published first
on the next run; published days are marked under
`$STATE_DIR/published/<archive>/`. `RECORDS_NOTICES` names the pollers' `_notices.json`, copied to the
archive root before signing so the notices are signed into every manifest.
Set it only for the archive whose pollers wrote it.

A day, once signed, is sealed: files added later are not in its manifest.
An archive that a batch job is still filling, such as
`hyperliquid-backfill` while `tools/hl-backfill process` runs in monthly
waves ([backfill](backfill.md)), sets `RECORDS_THROUGH` to the last day
the job has finished cleanly, and the pipeline signs nothing after it.

On a host without `deploy/up.sh`, `ARCHIVE_RUN` gives the command that runs
the archive CLI in the archive-job container, for example
`./bleh.sh --profile archive-jobs run --rm -T archive-job`.

### Live capture

`archive::follow` (used by the origin, and by `archive capture run
--follow --staging S --stream ... [--upload] --metrics-out F`):

- follows each stream's hourly node files as the node writes them and
  captures only complete lines (a partial last line of an older file, from
  a node crash, is kept verbatim in the private tier);
- seals an hour once every stream has a later file, or
  `--seal-grace-secs` (600) after the hour ended by the wall clock for a
  stopped node; node files rotate by hour, so a sealed hour never grows;
- uploads sealed hours write-once (with `--upload`) and writes
  `<staging>/.captured-through` (`YYYY-MM-DDTHH`): the last hour sealed in
  every stream and uploaded with nothing left in `ready/`; the node pruner
  (`deploy/scripts/hl-prune.sh`) deletes node output only up to it;
- rewrites `--metrics-out` every `--metrics-every-secs` (60), so the
  stale-capture alert (textfile age) sees a live process;
- resumes after a restart from the tiers' open hours (skipping the lines
  already written) and `<staging>/capture-state.json` (last sealed hour per
  stream).

Each manifest index refresh also writes `_manifests/latest.json`
(`{format, archive, date, sequence, manifest, sha256}`), which `publish`
uploads as a mutable object; monitoring probes it.

### Wiring (integrator)

The node reader is not a dependency of this crate. `archive::cli::Hooks`
carries:

- `filters: FilterSet` — a `LineFilter` (`Fn(&[u8]) -> Option<Vec<u8>>`)
  per stream; streams without one never reach the filtered tier;
- `parser: Option<Arc<dyn RawParser>>` — filtered raw files to records.

The `archive` binary is built by `crates/origin` (`src/bin/archive.rs`)
with `origin::hooks::env_hooks()`: the filters are
`hl_node_reader::filter_line` per stream, and the parser runs the reader's
replay and the origin's deriver, so replayed partitions hold exactly the
records the origin published live (trades, book events, derived records,
checkpoints, coverage). It reads `BLEH_ARCHIVE_REGISTRY` (required for
replay), `BLEH_ARCHIVE_BOOK_START` (`empty` for a first recorded day;
default `snapshot`) and `BLEH_BOOK_CHECKPOINT_SECS`. Without hooks
(`Hooks::default()`, library use), filtered capture and replay fail with
"not wired". Live capture uses the library directly:
`Capture::push(RawLine { stream, day, hour, line })` per node line,
`Capture::seal_before(day, hour)` once per wall-clock hour,
`HourWriter::position(stream)` to resume after a restart, and
`store::upload_ready` to ship sealed hours.

## 4. Object storage

Write-once uses conditional PUT (`If-None-Match: *`), supported by AWS S3,
Cloudflare R2 and MinIO. An existing identical object is accepted; a
different one is a conflict and is never overwritten. Files above 64 MiB go
up in parts after an existence check. Corrections never replace files:
`replay --correction` / `partition --correction` write the next part
number, listed in a new manifest (archive-layout § 6).

| Variable (`<TIER>` = `PUBLIC` or `PRIVATE`) | Meaning |
|---|---|
| `BLEH_ARCHIVE_<TIER>_URL` | `s3://bucket/prefix`, `file:///dir` or `memory://` |
| `BLEH_ARCHIVE_<TIER>_ENDPOINT` | R2: `https://<account>.r2.cloudflarestorage.com`; MinIO: `http://host:9000` |
| `BLEH_ARCHIVE_<TIER>_REGION` | Default `auto` (R2) |
| `BLEH_ARCHIVE_<TIER>_ACCESS_KEY_ID`, `..._SECRET_ACCESS_KEY` | Credentials; else the standard `AWS_*` variables |
| `BLEH_ARCHIVE_<TIER>_ALLOW_HTTP`, `..._PATH_STYLE` | `true` for a local MinIO |
| `BLEH_ARCHIVE_KUBO_API` | Kubo RPC API for `publish --kubo` |
| `BLEH_ARCHIVE_EVM_RPC`, `BLEH_ARCHIVE_EVM_KEY_FILE` | Defaults for the anchor commands |

Use separate buckets and credentials for the tiers: the private bucket
must not be readable publicly, and the publishing host needs no access to
it.

## 5. Keys

| Key | Use | Storage |
|---|---|---|
| Manifest signing key (secp256k1) | Signs `manifest.json` (EIP-191 over `keccak256("bleh-manifest-v0\n" ‖ canonical JSON)`) | File with 64 hex digits, mode 600, on the signing host only; address published in the registry |
| L2 anchor key (secp256k1) | Pays for and sends the anchor transaction | Separate file and funded account; holds only gas money |

- `archive manifest keygen --out FILE` creates a key from the OS
  randomness (mode 600, never overwrites) and prints its address. Keys are
  never printed or logged; a key file readable by others is warned about.
- Rotation: build the last manifest of the old key with
  `--next-signer 0x<new>`; the next manifest may be signed by either key;
  once one is signed by the new key the old one is retired. Publish both
  addresses in the registry first.
- Compromise: revoke the address in the registry; an unannounced signer
  change breaks the chain until maintainers record an emergency rotation.

## 6. Anchors

- OpenTimestamps: the digest is the manifest hash (= SHA-256 of
  `manifest.json`), submitted to the public calendars (at least two must
  accept). The stock client works too: `ots verify manifest.json.ots`.
  Upgrades contact only whitelisted calendars. Bitcoin attestations are
  verified against block headers from an Esplora-compatible API
  (`--bitcoin-api`, default suggestion `https://blockstream.info/api`).
- Ethereum L2 (chain to be decided): until the anchoring contract is
  specified, the transaction carries `anchor(bytes32 manifest_hash)` calldata
  (selector `keccak256("anchor(bytes32)")[..4]`), sent to `--to` or to the
  sender itself. `evm-poll` records the anchor once included, `confirmed`
  after `--confirmations` blocks. `--dry-run` signs without broadcasting and
  writes the raw transaction to `manifest.l2-tx.json`.
- Window (manifest-v0 § 6.3): missing or pending anchors are warnings for
  48 h (Bitcoin) and 24 h (L2), errors after that.

## 7. Cost knobs

| Knob | Effect |
|---|---|
| `capture run --private-level`, `--long-window-log`, `--dict` | Private-tier size versus CPU; dictionaries help small lines most |
| Filtered tier settings | Pinned (reproducibility); not a knob |
| Storage | R2 has no egress fees; keep the public bucket behind a CDN; the private bucket can use infrequent-access classes |
| IPFS | Pin on one always-on node; mirrors add redundancy at their own cost |
| L2 anchors | One transaction per archive per day (about 21,600 gas for a self-send); `--max-fee`, `--priority-fee`, `--gas-limit` cap the price |
| OpenTimestamps | Free; calendars aggregate submissions into one Bitcoin transaction |
| Verification | `--no-files` for chain and anchors only; `--deep` reads every byte and decodes every partition |

## 8. Mirroring and verifying (anyone)

1. Get the day's files from any channel: HTTPS (`<web seed><archive>/<path>`),
   the torrent (`magnets.txt` or the `.torrent` beside the manifest; clients
   fall back to the web seeds) or IPFS (`ipfs.cid` per file).
2. Get `manifest.json`, `manifest.anchors.json`, `manifest.json.ots` and
   `_manifests/index.json` (needed only to walk the chain over HTTPS).
3. Verify against a signer address from the registry (or a manifest hash
   you already trust, with `--checkpoint`):

   ```bash
   archive verify --dir ./hyperliquid-normalized --archive hyperliquid-normalized \
     --date 2026-01-01 --trusted 0x<registry signer> --deep \
     --bitcoin-api https://blockstream.info/api --evm-rpc https://<l2 rpc>
   ```

   The report lists every check as PASS, WARN or FAIL in the order
   signature, chain, anchors, files, and ends with `VERIFIED`,
   `VERIFIED WITH WARNINGS` or `FAILED`.
4. Serve or seed a file only after it passes; pin it under the manifest's
   CID and seed it under the day's infohash.
5. To add yourself as a web seed, serve the same tree at
   `https://<host>/<archive>/...` and ask for your base URL in future
   manifests.
6. To query with Iceberg from your own copy, rebuild the metadata for your
   location: `archive iceberg --root ./hyperliquid-normalized --location s3://your-bucket/hyperliquid-normalized`.

## 9. Known limits

- Instrument IDs may contain `#` (HIP-4 outcome symbols such as `#9001`).
  Partition paths write it as `%23` (`InstrumentId::path_segment`), so the
  object key holds a literal `%23` and HTTPS URLs escape the `%` (`%2523`).
  Readers that build URLs from paths must escape `%`.
- Book-dependent derived records (checkpoints, queue positions, heatmaps,
  order activity, liquidity grades) are replayed from the day's records
  alone, so they equal the live ones only when the replay starts from the
  same book: on the first recorded day (`BLEH_ARCHIVE_BOOK_START=empty`), or
  once L4 snapshots are captured into the raw tiers (not yet wired; see the
  Phase 2 plan). Trade-only analytics are day-local and always match.
- `ts_ingest_ns` is a nanosecond Parquet timestamp; Iceberg v2 has no
  nanosecond type, so the tables declare `timestamptz` (microseconds) and
  readers convert.
- The Iceberg `date` partition is `day(ts_source_ms)` (`day(start_ms)` for
  interval kinds), which equals the Hive `date=` value; the spec's identity
  partition on `date` is not possible because `date` is not a column.
- Mirror verification over HTTPS holds each file in memory while hashing.

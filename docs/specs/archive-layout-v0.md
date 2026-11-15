# Archive layout, v0

- Status: Draft; implemented by `crates/archive`
- Applies to: the filtered raw archive and the normalized partitions

History is **static first**: plain files on object storage that common
engines query directly, with no database or API in the path. The same files
are listed in the daily manifests ([manifest-v0.md](manifest-v0.md)) and
seeded over BitTorrent and IPFS. The historical API (`crates/history-api`)
is a convenience on top, not a requirement.

## 1. Archives

| Archive | Contents | Format |
|---|---|---|
| `hyperliquid-raw` | Node output with excluded fields and streams removed | JSON lines, zstd |
| `hyperliquid-normalized` | Records ([observation-v0](observation-v0.md)), one table per kind | Parquet |
| `reference-normalized` | Reference values and positioning from the reference pollers ([ADR 0017](../decisions/0017-records-archives.md)) | Parquet |
| `swaps-normalized` | Swap prints from the swap pollers ([ADR 0017](../decisions/0017-records-archives.md)) | Parquet |

Each archive has its own manifest chain. A backfill from another source is
a separate archive with its own admission ID (for example
`hyperliquid-backfill`), and its records carry the `backfill` flag.

## 2. Filtered raw archive

Node files contain fields and events ADM-0001 excludes (owner addresses,
liquidation details, fees, spot markets, non-book fills), so raw files are
never published as written. The capture step passes every line through
the node reader's filter, which defines the filtered-raw format:
[hyperliquid-node-format.md](hyperliquid-node-format.md) § 12. In short,
it drops streams other than fills, order statuses and raw book diffs,
keeps every block line of those three (even with no events left), and
keeps only admitted events and fields, as compact JSON. The replay tool
rebuilds normalized partitions from these files alone.

Layout:

```text
hyperliquid-raw/stream=<stream>/date=<YYYY-MM-DD>/<HH>.jsonl.zst
```

`<stream>` is `fills`, `order_statuses` or `raw_book_diffs`; `<HH>` is the
two-digit UTC hour of the node file. Compression: zstd level 19, one frame,
no dictionary, content size and checksum flags on.

## 3. Normalized partitions

### 3.1 Paths

Hive-style, so engines derive partition columns from paths:

```text
hyperliquid-normalized/kind=<kind>/venue=<venue>/instrument=<id>/date=<YYYY-MM-DD>/part-<NNNNN>.parquet
```

- `kind` is the record kind (`trade`, `book_event`, `book_snapshot`, `bar`,
  ...).
- `instrument` is `InstrumentId::path_segment()`: the ID with `:` written as
  `~` and `#` written as `%23` (`hyperliquid~perp~xyz~CL`,
  `hyperliquid~outcome~%239001`). Readers reverse both; a raw `#` is also
  accepted. The object key holds the literal `%23`, so an HTTPS URL for
  it escapes the `%` (`%2523`). The `instrument` column inside the file
  keeps the real ID. Venue-wide records (`coverage`) use
  `hyperliquid~venue~all`.
- `date` is the UTC day of `ts_source_ms`; for interval records, of the
  interval start (`start_ms` of bars, footprints, volume profiles, delta
  bars, value areas, order activity and coverage; `time_start_ms` of
  heatmap tiles; `first_ms` of activity bars).
- `part-<NNNNN>` is numbered from `00000`. A new part starts after
  16,777,216 rows (2^24), so part boundaries depend only on row count.

### 3.2 File contents

| Setting | Value |
|---|---|
| Schema | `mesh_types::arrow::schema(kind)` |
| Row order | Canonical: `instrument`, then `block_number`, then `event_index` (that is, by `seq`) |
| Compression | zstd, level 9, every column |
| Row groups | Exactly 131,072 rows (2^17), except the last |
| Data pages | 1 MiB size limit, data page v1, dictionary encoding on for `Utf8` columns |
| Statistics | Min and max per row group and per page (page index on), so engines prune by `block_number` and time |
| Bloom filters | Off |
| `created_by` | `bleh-archive` (fixed: no library or version string) |
| Key-value metadata | Exactly `bleh.schema`, `bleh.kind`, `bleh.content_hash`, `bleh.record_count`; the Arrow IPC schema blob is not written |

`bleh.content_hash` is the canonical content hash of the file's records in
file order, as also listed in the manifest. It is independent of the
Parquet bytes, so it is the reproducibility contract: replaying the raw
archive with any correct implementation must produce records with the same
content hash. Byte-identical files are additionally guaranteed for a fixed
version of the archive crate and its Parquet library, and CI checks this on
a fixture day.

**Schema changes are additive only.** A kind's schema may gain nullable
columns, appended after the existing ones, so every existing column keeps
its position and Parquet field ID and the new column takes the next ID.
Partitions are write-once and are not rewritten when a column is added:

- The archive's reader takes a nullable column that is absent from a file
  as null, so an older file decodes to the same records and the same
  `bleh.content_hash`.
- The Iceberg metadata (§ 3.3) is rebuilt with the current schema; a field
  ID absent from a data file reads as null, Iceberg's rule for added
  columns.
- DuckDB reads older and newer files of a kind together with
  `read_parquet([...], union_by_name = true)`, or through `iceberg_scan`.

Added so far: `reference_value.unit` (field ID 20), the unit of a quantity
([ADR 0023](../decisions/0023-reference-quantities.md)). `reference-normalized`
files written before it have no `unit` column; every record in them is a
rate, yield, price or index level, for which `unit` is null. Quantity rows
are told apart by `measure` (`stock`, `flow`, `capacity`, `ratio`) and
`unit`, so a query needs no lookup:

```sql
SELECT instrument, as_of_date, value, unit
FROM read_parquet('reference-normalized/kind=reference_value/venue=eia/**/*.parquet',
                  hive_partitioning = true, union_by_name = true)
WHERE measure = 'stock' AND unit = 'thousand_barrels';
```

### 3.3 Catalog: Apache Iceberg, static metadata

**Recommendation: Apache Iceberg (table format v2), with metadata files
published next to the data and no catalog service.**

```text
hyperliquid-normalized/_iceberg/<kind>/metadata/v<N>.metadata.json
hyperliquid-normalized/_iceberg/<kind>/metadata/version-hint.text
hyperliquid-normalized/_iceberg/<kind>/metadata/<snapshot manifests>.avro
```

- One table per kind, identity-partitioned on `venue`, `instrument` and
  `date`, referencing the Parquet files above in place (files are added,
  never rewritten).
- A new table snapshot is committed when a day's manifest is published.
  `version-hint.text` names the latest metadata file.
- Readers open the table from `v<N>.metadata.json` directly; no REST catalog
  is needed.

Why Iceberg over DuckLake: Iceberg metadata is plain files that DuckDB
(`iceberg` extension), Polars (`scan_iceberg`), ClickHouse (`icebergS3`
table function), Spark and Trino all read without a server. DuckLake needs a
SQL catalog database and is read mainly by DuckDB today. DuckLake can be
added later over the same Parquet files if it gains broad support.

The Iceberg catalog is optional for readers: the Hive paths and the
manifests' file lists work without it.

## 4. Querying

Manifests live under the archive root, one directory per day:

```text
<archive>/_manifests/<YYYY-MM-DD>/manifest.json
<archive>/_manifests/<YYYY-MM-DD>/manifest.anchors.json
<archive>/_manifests/<YYYY-MM-DD>/manifest.json.ots
```

Three ways to find files, all over the same bytes:

1. **Object-store listing** (S3 API): glob the Hive paths.
2. **Iceberg metadata**: open the table's metadata file.
3. **Manifest file list**: plain HTTPS, torrents and IPFS cannot list
   directories, so read `manifest.json` and query the listed paths. This is
   what DuckDB-WASM in a browser uses.

Examples (bucket URL illustrative):

```sql
-- DuckDB, httpfs over S3 with Hive partitioning
SELECT ts_source_ms, price / 10^price_scale AS px, size / 10^size_scale AS sz, aggressor
FROM read_parquet('s3://bucket/hyperliquid-normalized/kind=trade/venue=hyperliquid/instrument=hyperliquid~perp~BTC/date=*/*.parquet',
                  hive_partitioning = true)
WHERE date = '2026-01-01'
ORDER BY block_number, event_index;

-- DuckDB, Iceberg
SELECT count(*) FROM iceberg_scan('s3://bucket/hyperliquid-normalized/_iceberg/trade/metadata/v42.metadata.json');
```

```python
# Polars
import polars as pl
lf = pl.scan_parquet("s3://bucket/hyperliquid-normalized/kind=bar/**/*.parquet", hive_partitioning=True)
```

```sql
-- ClickHouse
SELECT count() FROM s3('https://bucket.example/hyperliquid-normalized/kind=trade/**/*.parquet', 'Parquet');
SELECT count() FROM icebergS3('https://bucket.example/hyperliquid-normalized/_iceberg/trade/');
```

DuckDB-WASM: fetch the day's `manifest.json`, then
`read_parquet([...listed https URLs...])`. Page indexes and fixed row
groups keep range requests small.

## 5. Distribution

A day's files are those whose `date=` (raw and normalized) is that day.
They are exactly the file set of that day's manifest, its torrent and its
IPFS content; paths in the manifest are relative to the archive root, so
the Hive layout is the same on every channel. The `_iceberg/` metadata is
published over HTTPS and object storage only; it is derived and can be
rebuilt from the manifests.

## 6. Immutability and corrections

Files are write-once. A correction writes a new file under a new part
number or a new archive version, in a new manifest; old files stay
addressable by hash, CID and infohash. Gaps are recorded (flags and the
coverage map), never filled with synthetic or interpolated data.

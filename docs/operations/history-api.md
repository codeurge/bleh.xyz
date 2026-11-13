# History API runbook

- Crate: [`crates/history-api`](../../crates/history-api)
- Reads: the normalized archive ([archive-layout-v0](../specs/archive-layout-v0.md)),
  records in [observation-v0](../specs/observation-v0.md), manifests in
  [manifest-v0](../specs/manifest-v0.md)
- Speaks: JSON (`/v0`), websocket replay in [gateway-v0](../protocol/gateway-v0.md),
  Databento-shaped DBN ([ADR 0004](../decisions/0004-wire-format.md)),
  TradingView UDF, a CCXT-style shim and MCP

History is **static first** ([ADR 0006](../decisions/0006-phase-1-2-service-architecture.md)):
the Parquet files, manifests, torrents and CIDs are the primary interface,
and anyone can query them without this server (see
[Querying the files directly](#querying-the-files-directly)). This service is
a convenience for small JSON queries, replay and compatibility layers. It
reads precomputed bars, footprints and volume profiles; it never recomputes
them.

## Quick start (synthetic demo)

```bash
cargo run --release -p history-api -- demo
```

`demo` writes a small **synthetic** archive (two fictional markets,
`hyperliquid:perp:SYN` and `hyperliquid:perp:syn:IDX`, four hours from
2026-01-01T22:00Z across a UTC day boundary, one deliberately missing hour on
`syn:IDX`) to a temporary directory and serves it on `127.0.0.1:8080`. Every
value is invented by a seeded simulation. It prints example requests.

`history-api synth --dir ./synthetic` writes the same archive without
serving it, for experimenting with DuckDB or Polars.

Build time: DuckDB is compiled from source (the `bundled` feature), which
takes about 5 minutes of wall time for the first debug build on an 8-core
laptop (about 27 CPU-minutes); later builds reuse it.

## Running

```bash
history-api serve --archive /srv/archive --registry /srv/registry.json --bind 0.0.0.0:8080
history-api serve --archive s3://bucket/prefix --s3-region auto --s3-endpoint <account>.r2.cloudflarestorage.com
history-api serve --archive https://data.example --public-base-url https://data.example
```

| Flag | Environment | Default | Meaning |
|---|---|---|---|
| `--archive` | `HISTORY_API_ARCHIVE` | required | Local directory, `s3://`/`r2://`/`gcs://` URL, or HTTPS base |
| `--archive-name` | `HISTORY_API_ARCHIVE_NAMES` | `hyperliquid-normalized` | Archive names under the root, unioned (for example live plus backfill) |
| `--registry` | `HISTORY_API_REGISTRY` | none | Registry document ([registry-v0](../specs/registry-v0.md), `mesh_types::Registry`); without it instruments are discovered from partitions |
| `--public-base-url` | `HISTORY_API_PUBLIC_BASE_URL` | the root, for HTTPS roots | Base for links in `/v0/manifests` |
| `--bind` | `HISTORY_API_BIND` | `127.0.0.1:8080` | Listen address |
| `--trust-forwarded-for` | `HISTORY_API_TRUST_FORWARDED_FOR` | off | Rate-limit by the first `X-Forwarded-For` address (only behind a proxy that sets it) |
| `--mcp-allowed-host` | `HISTORY_API_MCP_ALLOWED_HOSTS` | `localhost,127.0.0.1,::1` | `Host` values `/mcp` accepts (DNS-rebinding protection); set to the public host name in production |
| `--extension-dir` | `HISTORY_API_EXTENSION_DIR` | none | Pre-downloaded DuckDB extensions (offline, reproducible deployments) |
| `--s3-endpoint`, `--s3-region`, `--s3-url-style-path` | `HISTORY_API_S3_*` | none | S3-compatible stores (R2, MinIO). Credentials come from `AWS_ACCESS_KEY_ID` and `AWS_SECRET_ACCESS_KEY` when set; public buckets need none |
| `--log-format` | `HISTORY_API_LOG_FORMAT` | `text` | `text` or `json`; logs go to stderr, `RUST_LOG` filters |

Remote roots need DuckDB's `httpfs` extension, loaded at start-up (from
`--extension-dir`, or installed from DuckDB's extension repository on first
use). It is not compiled in because it would link OpenSSL, which
`deny.toml` bans. Local roots load no extensions at all.

How files are found: a local root is walked; an object store is globbed
(`glob()` returns nothing for a missing day rather than failing); an HTTPS
root cannot list, so the day's `_manifests/<date>/manifest.json` supplies
the file list. Data for a day is therefore visible over HTTPS only once its
manifest is published.

## Limits

Every limit is enforced server-side and published at `GET /v0/limits`.

| Limit | Flag | Default |
|---|---|---|
| Query timeout (the query is interrupted, `504 query_timeout`) | `--query-timeout-ms` | 10 s |
| DuckDB memory cap, no spilling (`413 too_large`) | `--memory-limit-mb` | 1 GiB |
| DuckDB threads | `--threads` | 4 |
| Concurrent queries (more wait 5 s, then `503 overloaded`) | `--max-concurrent-queries` | 8 |
| Page size default / maximum | `--default-rows` / `--max-rows` | 1,000 / 10,000 |
| Widest range for records and coverage | `--max-range-days` | 31 days |
| Widest range for bars | (derived) | the larger of the above and one page of bars (10,000 × interval) |
| Book checkpoint lookback | `--checkpoint-lookback-hours` | 24 h |
| Book events applied on a checkpoint | `--max-book-events` | 2,000,000 |
| Replay range / concurrent replays per IP | `--max-replay-days` / `--max-replays-per-ip` | 7 days / 2 |
| Records per Databento-shaped request | `--max-dbn-records` | 1,000,000 |
| Requests per IP (token bucket; `/health` and `/metrics` exempt) | `--rate-per-sec` / `--rate-burst` | 20/s, burst 50 |

Defence in depth: SQL is built only by the service, user values are bound
parameters, and DuckDB's configuration is locked after start-up
(`lock_configuration`). Local roots run with `enable_external_access =
false` and `allowed_directories` set to the archive root; remote roots
disable DuckDB's local filesystem.

## Endpoints

Times accept milliseconds since the epoch, RFC 3339 (`2026-01-01T22:00:00Z`)
or a date (`2026-01-01`, UTC midnight). Ranges are `[start, end)`. Records
are observation-v0 JSON: decimals are strings with exactly the instrument's
scale (`"100.250"`), `*_ns` are strings. Errors are always:

```json
{"error": {"code": "range_too_large", "message": "range is wider than the maximum of 744 hours; split the query"}}
```

Codes: `bad_request`, `not_found`, `unknown_instrument`,
`unsupported_interval`, `unsupported`, `range_too_large`, `invalid_cursor`,
`rate_limited` (with `Retry-After`), `overloaded`, `query_timeout`,
`too_large`, `archive_unavailable`, `internal`.

Pagination: list endpoints return `next_cursor`; pass it back as `cursor`
with the same other parameters. Cursors are opaque and bound to the query
that produced them (`invalid_cursor` otherwise).

| Endpoint | Purpose |
|---|---|
| `GET /health` | `200 {"status":"ok", "duckdb": ...}` when the engine answers |
| `GET /metrics` | Prometheus: requests by route and status, request and query latency histograms, rate-limit rejections, rows served, active replays, busy query slots |
| `GET /v0/openapi.json` | OpenAPI 3.1 document |
| `GET /v0/limits` | Limits in force |
| `GET /v0/instruments?q=&venue=&class=&limit=&cursor=` | Registry and archive instruments: scales, flags, admission, kinds and dates present |
| `GET /v0/trades?instrument=&start=&end=&limit=&cursor=` | Trades in block order (default range: one hour) |
| `GET /v0/book-events?instrument=&start=&end=&limit=&cursor=` | Order-level book events in block order |
| `GET /v0/bars?instrument=&interval=&start=&end=&limit=&cursor=` | Precomputed bars: `1s`, `1m`, `5m`, `15m`, `1h`, `1d` |
| `GET /v0/footprint?instrument=&interval=&start=&end=&limit=&cursor=` | Precomputed footprints |
| `GET /v0/volume-profile?instrument=&start=&end=&limit=&cursor=` | Precomputed volume profiles overlapping the range |
| `GET /v0/book?instrument=&at=&depth=l4\|l2&levels=` | Book at time T (default: latest data) |
| `GET /v0/coverage?instrument=&start=&end=` | Recorded, backfilled and missing hours per market |
| `GET /v0/manifests?start=&end=&archive=&files=true` | Signed manifests with magnet, infohash, CIDs and file URLs |
| `GET /v0/replay?start=&end=&speed=` | Websocket replay (below) |
| `/databento/v0/...`, `/udf/...`, `/ccxt/...`, `/mcp` | Compatibility layers (below) |

Examples (demo archive):

```bash
curl 'http://127.0.0.1:8080/v0/trades?instrument=hyperliquid:perp:SYN&start=2026-01-01T22:00:00Z&limit=2'
```

```json
{"instrument":"hyperliquid:perp:SYN","kind":"trade","start_ms":1767304800000,"end_ms":1767308400000,"count":2,
 "records":[
  {"kind":"trade","prov":{"instrument":"hyperliquid:perp:SYN","venue":"hyperliquid","ts_source_ms":1767304800000,
   "ts_ingest_ns":"1767304800004503642","seq":{"block_number":5000000,"event_index":0},"admission":"ADM-0001","flags":[]},
   "price":"99.990","size":"4.125","aggressor":"sell","trade_id":900000001},
  {"kind":"trade","prov":{"instrument":"hyperliquid:perp:SYN","venue":"hyperliquid","ts_source_ms":1767304800000,
   "ts_ingest_ns":"1767304800004503642","seq":{"block_number":5000000,"event_index":1},"admission":"ADM-0001","flags":[]},
   "price":"99.990","size":"0.019","aggressor":"sell","trade_id":900000002}],
 "next_cursor":"eyJ2IjoxLCJxIjoidHJhZGV8aHlwZXJsaXF1aWQ6cGVycDpTWU58MTc2NzMwNDgwMDAwMHwxNzY3MzA4NDAwMDAwIiwiayI6WzUwMDAwMDAsMV19"}
```

```bash
curl 'http://127.0.0.1:8080/v0/book?instrument=hyperliquid:perp:SYN&at=2026-01-01T22:30:00Z&depth=l2&levels=2'
```

```json
{"instrument":"hyperliquid:perp:SYN","at_ms":1767306600000,
 "checkpoint":{"seq":{"block_number":5000899,"event_index":4294967295},"ts_source_ms":1767306598000,"depth":"l4","flags":[]},
 "events_applied":5,"inconsistencies":{"unknown_order":0,"duplicate_add":0},
 "best_bid":"99.989","best_ask":"100.022",
 "book":{"kind":"book_snapshot","prov":{"...":"..."},"depth":{"type":"l2","bids":[{"price":"99.989","size":"...","order_count":1}],"asks":["..."]}}}
```

```bash
curl 'http://127.0.0.1:8080/v0/bars?instrument=hyperliquid:perp:SYN&interval=5m&start=2026-01-01T22:00:00Z'
curl 'http://127.0.0.1:8080/v0/coverage?instrument=hyperliquid:perp:syn:IDX&start=2026-01-01T22:00:00Z&end=2026-01-02T02:00:00Z'
curl 'http://127.0.0.1:8080/v0/manifests?start=2026-01-01&end=2026-01-03&files=true'
```

### Book at time T

The book is the nearest checkpoint (`book_snapshot`) at or before T within
the lookback, plus every `book_event` with a greater `seq` and
`ts_source_ms <= T` (observation-v0 § 2.4). The result is a `book_snapshot`
record at the end of the last block at or before T, with the checkpoint
used, the number of events applied and any inconsistencies (events for
unknown orders are counted, never invented). An L2 checkpoint (for example a
backfilled hourly snapshot) is returned as it is, since order-level events
cannot be applied to it. No checkpoint in the lookback is `404 not_found`.

### Coverage

An hour is `recorded` when the archive has a trade or book event for the
market in it, `backfilled` when all of them carry the `backfill` flag, and
`missing` otherwise. A quiet market and a recording gap look alike unless a
record carries `gap_before` (reported per hour). Gaps are never filled.

## Websocket replay

```text
ws://host/v0/replay?start=2026-01-01T22:00:00Z&end=2026-01-01T23:00:00Z&speed=10
```

The server sends the gateway-v0 `hello`; the client sends an ordinary
gateway-v0 `subscribe`:

```json
{"op":"subscribe","id":"1","channels":[
  {"instrument":"hyperliquid:perp:SYN","kind":"trade"},
  {"instrument":"hyperliquid:perp:SYN","kind":"book_event","depth":"l4"},
  {"instrument":"hyperliquid:perp:SYN","kind":"bar","interval":"1m"}]}
```

Then, exactly as live: a `snapshot` per book channel (reconstructed at
`start`), and per block `batch` messages (a `BlockBatch` with
`prev_block_number` links), then `record` messages for bars whose interval
has closed by that block (never earlier, so backtests see no look-ahead),
then a `block` heartbeat with `last`. When the range ends the server closes
with code 1000. `speed` multiplies archive time (`1` real time, `0` as fast
as the connection allows, maximum 1,000). `ping`, `unsubscribe` and
`resnapshot` work during the replay.

Differences from the live gateway: heartbeats are sent only for blocks with
records in a subscribed channel (the archive does not record empty blocks);
`ts_publish_ns` is the replay send time; commitments and proofs are not
served (verify files against the manifests instead); JSON encoding only;
channels are fixed at the first `subscribe`.

## Compatibility layers

### Databento-shaped historical API (DBN)

```bash
curl -o syn.dbn 'http://127.0.0.1:8080/databento/v0/timeseries.get_range?dataset=BLEH.HL&symbols=hyperliquid:perp:SYN&schema=trades&start=2026-01-01T22:00:00Z&end=2026-01-01T23:00:00Z'
```

Same parameters as Databento's `timeseries.get_range` (`GET` with a query
string or `POST` with a form body): `dataset`, `symbols` (comma-separated or
`ALL_SYMBOLS`), `schema`, `start`, `end`, `stype_in=raw_symbol`, `limit`,
`encoding` (`dbn`, `json`, `csv`), `compression` (`none`, `zstd`),
`pretty_px`, `pretty_ts`. Also `metadata.list_datasets`,
`metadata.list_schemas` and `metadata.list_publishers`. Encoding uses the
Apache-2.0 `dbn` crate (DBN version 3), so any DBN reader decodes the
output:

```python
import databento as db
store = db.DBNStore.from_file("syn.dbn")
df = store.to_df()
```

| bleh | DBN | Notes |
|---|---|---|
| Venue `hyperliquid` | dataset `BLEH.HL`, publisher ID 45001 | bleh-chosen IDs from a private range, not assigned by Databento; no affiliation implied |
| Instrument ID (`hyperliquid:perp:BTC`) | `raw_symbol` in; `instrument_id` out | 32-bit ID = first four bytes of SHA-256 of the instrument ID, listed in the metadata symbol mappings; collisions in one request are refused |
| `price` mantissa and scale | `price`, 1e-9 fixed point | Exact for scales up to 9 (Hyperliquid uses at most 8); above 9 is rounded |
| `size` mantissa and scale | `size` (u32), `volume` (u64) | **Lossy point.** DBN sizes are integers. `size_units=lots` (default, lossless) writes the mantissa, that is the size in units of `10^-size_scale`; scales are in the `X-Bleh-Size-Scales` header. `size_units=whole` rounds to whole units, losing fractional sizes. Values above `u32::MAX` saturate, counted in `X-Bleh-Size-Saturated` |
| `ts_source_ms` | `ts_event` (ns) | Millisecond precision; the mesh does not invent digits |
| `ts_ingest_ns` | `ts_recv` | Node receive time |
| `seq.block_number` | `sequence` | **Lossy**: modulo 2^32; `event_index` is not carried |
| Last record of a block | `F_LAST` | |
| `trade` | `trades` (`action=T`; `side` B = buy aggressor, A = sell, N = unknown) | |
| `book_event` | `mbo` | Starts with a clear (`R`) and the book at `start` as `A` records flagged `F_SNAPSHOT`. `add` = `A`; `modify` = `M`; `fill` = `F` (filled quantity) then `M` (remaining) or `C` (removal); `cancel` = `C` with the removed size |
| `book_event` | `mbp-1`, `mbp-10` | Events that touch the top 1 or 10 levels, with the levels after the event. Trades are not interleaved: the archive does not order trades against book events within a block (observation-v0 § 3) |
| `bar` 1s, 1m, 1h, 1d | `ohlcv-1s`, `-1m`, `-1h`, `-1d` | `ts_event` is the bar start. 5m and 15m bars have no DBN schema |
| `prov.admission`, `prov.flags`, `derived` header, signatures | not carried | Use `/v0/...` or the manifests for provenance |

Not offered: `tbbo`, `bbo-*`, `cbbo-*`, `definition`, `statistics`,
`status`, `imbalance`, live DBN.

### TradingView UDF datafeed

Point the Charting Library's `UDFCompatibleDatafeed` at
`http://host/udf`. Endpoints: `/udf/config`, `/udf/symbols?symbol=`,
`/udf/search?query=&type=&exchange=&limit=`,
`/udf/history?symbol=&resolution=&from=&to=&countback=`, `/udf/time`.
Resolutions `1S`, `1`, `5`, `15`, `60`, `1D` map to the precomputed bars.
Symbols are instrument IDs; `pricescale` is `10^price_scale`. **Lossy
point:** UDF requires JSON numbers, so prices and volumes are `f64` here.
Without data in the window, `history` answers `no_data` with `nextTime`
(the latest earlier bar).

### CCXT-style REST shim

| Endpoint | CCXT method |
|---|---|
| `GET /ccxt` | `describe()` (capabilities, timeframes) |
| `GET /ccxt/markets` | `fetchMarkets()` |
| `GET /ccxt/trades?symbol=&since=&limit=` | `fetchTrades(symbol, since, limit)`; most recent without `since` |
| `GET /ccxt/ohlcv?symbol=&timeframe=&since=&limit=` | `fetchOHLCV`; `[[t, o, h, l, c, v], ...]` |
| `GET /ccxt/orderbook?symbol=&limit=&timestamp=` | `fetchOrderBook`; book at `timestamp` (default latest), `nonce` = block |
| `GET /ccxt/time` | `fetchTime()` |

Structures follow CCXT's unified shapes. Symbols are instrument IDs (the
shim does not invent `BASE/QUOTE:SETTLE` names the registry does not
carry); numbers are `f64`, and each trade's `info` holds the exact
observation-v0 record.

### MCP server

Tools: `list_instruments`, `get_trades`, `get_bars`, `get_book_at`,
`get_volume_profile`, `get_coverage` (runs of equal status),
`explain_provenance` (the record, its canonical encoding, admission, flags,
derivation method, archive file, manifest entry and verification steps).
Pages are capped at 500 records. Built on the official Rust SDK (`rmcp`).

Stdio (Claude Desktop, Claude Code and other clients):

```json
{
  "mcpServers": {
    "bleh-history": {
      "command": "history-api",
      "args": ["mcp", "--archive", "https://data.example"]
    }
  }
}
```

```bash
claude mcp add bleh-history -- history-api mcp --archive /srv/archive
```

Streamable HTTP: the server mounts it at `/mcp` on the same port:

```bash
claude mcp add --transport http bleh-history https://history.example/mcp
```

Set `--mcp-allowed-host history.example` so the DNS-rebinding check accepts
the public host name.

## Querying the files directly

No server needed. Paths and columns are in
[archive-layout-v0](../specs/archive-layout-v0.md) and
[observation-v0 § 9](../specs/observation-v0.md#9-arrow-schemas); bucket and
host names below are placeholders. Prices and sizes are integer mantissas
with `price_scale` and `size_scale` columns.

DuckDB, local or S3/R2 with Hive partitioning:

```sql
INSTALL httpfs; LOAD httpfs;   -- remote only
SELECT ts_source_ms,
       price / 10^price_scale AS px,
       size / 10^size_scale  AS sz,
       aggressor
FROM read_parquet('s3://bucket/hyperliquid-normalized/kind=trade/venue=hyperliquid/instrument=hyperliquid~perp~BTC/date=*/*.parquet',
                  hive_partitioning = true)
WHERE date BETWEEN '2026-01-01' AND '2026-01-02'
ORDER BY block_number, event_index;
```

DuckDB over plain HTTPS (no listing): take file paths from the manifest.

```sql
SET VARIABLE files = (
  SELECT list('https://data.example/hyperliquid-normalized/' || f.path)
  FROM (SELECT unnest(manifest.files) AS f
        FROM read_json('https://data.example/hyperliquid-normalized/_manifests/2026-01-01/manifest.json'))
  WHERE f.path LIKE 'kind=bar/%instrument=hyperliquid~perp~BTC/%');
SELECT start_ms, open / 10^price_scale AS open, close / 10^price_scale AS close
FROM read_parquet(getvariable('files')) WHERE interval = '1h' ORDER BY start_ms;
```

Book at time T, by hand (the checkpoint, then events after it):

```sql
WITH cp AS (
  SELECT block_number FROM read_parquet('archive/hyperliquid-normalized/kind=book_snapshot/venue=hyperliquid/instrument=hyperliquid~perp~BTC/date=2026-01-01/*.parquet')
  WHERE ts_source_ms <= TIMESTAMPTZ '2026-01-01 12:00:00+00' ORDER BY block_number DESC LIMIT 1)
SELECT * FROM read_parquet('archive/hyperliquid-normalized/kind=book_event/venue=hyperliquid/instrument=hyperliquid~perp~BTC/date=2026-01-01/*.parquet')
WHERE block_number > (SELECT block_number FROM cp)
  AND ts_source_ms <= TIMESTAMPTZ '2026-01-01 12:00:00+00'
ORDER BY block_number, event_index;
```

DuckDB with the Iceberg catalog, when published:

```sql
INSTALL iceberg; LOAD iceberg;
SELECT count(*) FROM iceberg_scan('s3://bucket/hyperliquid-normalized/_iceberg/trade/metadata/v42.metadata.json');
```

Polars:

```python
import polars as pl
bars = (
    pl.scan_parquet("s3://bucket/hyperliquid-normalized/kind=bar/**/*.parquet", hive_partitioning=True)
    .filter((pl.col("instrument") == "hyperliquid:perp:BTC") & (pl.col("interval") == "1m"))
    .with_columns((pl.col("close") / 10 ** pl.col("price_scale")).alias("close_px"))
    .sort("start_ms")
    .collect()
)
```

ClickHouse:

```sql
SELECT toStartOfHour(ts_source_ms) AS h, count() AS trades, sum(size / pow(10, size_scale)) AS volume
FROM s3('https://bucket.example/hyperliquid-normalized/kind=trade/venue=hyperliquid/instrument=hyperliquid~perp~BTC/date=2026-01-*/*.parquet', 'Parquet')
GROUP BY h ORDER BY h;
```

Verify what you downloaded: check each file's SHA-256 against the day's
signed `manifest.json` (signature and hash chain per
[manifest-v0](../specs/manifest-v0.md)), or fetch it by `ipfs.cid` or the
day's magnet link, which commit to the same bytes.

## Operations

- **Health:** alert when `/health` is not 200, and on
  `bleh_history_query_seconds` p99 near the query timeout.
- **Overload:** `bleh_history_query_slots_busy` at the cap with `overloaded`
  errors means raise `--max-concurrent-queries` (and memory) or add
  replicas; the service is stateless and scales horizontally behind a load
  balancer.
- **Memory:** the DuckDB cap applies to the whole process's engine; size the
  container at roughly the cap plus 256 MiB.
- **Abuse:** per-IP rate limits and replay caps; behind a proxy, set
  `--trust-forwarded-for` so limits key on the client, not the proxy.
- **Upgrades:** stateless; roll replicas. Replays in progress end with a
  close frame and clients reconnect.

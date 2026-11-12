# Bring your own feed

User guide for `mesh-client byo`: merge a feed you license yourself with
bleh.xyz data **on your own machine**, and measure how bleh.xyz prices track
it. Decision record: [ADR 0014](../decisions/0014-user-supplied-data-stays-local.md).
Client runbook: [client.md](client.md).

## Your license, your machine

- **You are responsible for your own license.** bleh.xyz grants no rights to
  your feed and checks none. Use it only as your agreement with your
  provider allows.
- **bleh.xyz never receives your data.** The adapter reads your feed
  locally and subscribes to a bleh.xyz gateway read-only. Nothing from your
  feed, and nothing computed from it, is sent to any bleh.xyz service,
  signed, archived or published. The project ships the connector code only.
- Every output line built from your data carries
  `"provenance":"user_supplied"`, so you can tell it apart from bleh.xyz
  data (which is CC0) wherever it ends up.
- The adapter refuses to write output inside a bleh.xyz archive directory
  or a bleh.xyz source checkout, and its websocket listens on loopback only.

## Quick start

1. Write a mapping file that pairs your symbols with bleh.xyz markets:

   ```toml
   [[pair]]
   name = "btc-perp-vs-mine"        # unique; names the pair in all outputs
   user_symbol = "MY-BTC"           # the symbol as your feed writes it
   user_price = "any"               # any (trade, else mid) | trade | mid
   multiplier = 1.0                 # your price x multiplier is compared

   [pair.bleh]
   market = "hyperliquid:perp:BTC"  # a bleh.xyz instrument ID
   price = "mid"                    # trade | mid | bar_close
   # interval = "1m"                # bar_close only
   ```

2. Check it, and see the gateway channels it needs:

   ```bash
   mesh-client byo check-map --map pairs.toml
   {"channels":["hyperliquid:perp:BTC|book_event|l2"],"inactive":[],"ok":true,"pairs":1}
   ```

3. Merge, live:

   ```bash
   mesh-client byo merge --map pairs.toml \
     --url wss://a.example/v0/ws --url wss://b.example/v0/ws \
     --feed csv --feed-path - --ts-format ms \
     --stats tracking.jsonl --stats-every 1m < my-export-pipe
   ```

   or offline, against a bleh.xyz recording made with
   `mesh-client --url ... --sub <each channel> --record bleh.jsonl`:

   ```bash
   mesh-client byo merge --map pairs.toml --recording bleh.jsonl \
     --feed dbn --feed-path my-file.dbn.zst \
     --out merged.jsonl --stats tracking.jsonl
   ```

Pair files with recordings and live feeds with a live gateway: a file of
last month's data cannot align with today's live stream (the adapter warns
when the two sides start more than a day apart).

## Mapping file

| Key | | |
|---|---|---|
| `name` | required | Unique pair name |
| `user_symbol` | required | Symbol in your feed (DBN: the raw symbol from the file's symbology, or the numeric instrument ID if unmapped) |
| `user_price` | `any` | `trade` uses trade prices only, `mid` uses bid/ask mids only, `any` takes the trade price and falls back to the mid |
| `multiplier` | `1.0` | Applied to your price before comparison (units, contract sizes) |
| `[pair.bleh] market` | one of three | A bleh.xyz instrument ID, served by gateway v0 |
| `[pair.bleh] price` | `trade` | `trade`, `mid` (L2 book), or `bar_close` with `interval` (`1s`, `1m`, `5m`, `15m`, `1h`, `1d`) |
| `[pair.bleh] synthetic_month` | one of three | Reserved: a synthetic contract month ID |
| `[pair.bleh] exposure` | one of three | Reserved: an exposure's front series |

`synthetic_month` and `exposure` pairs validate today but stay inactive
(`check-map` lists them under `inactive`, `merge` skips them with a note)
until the synthetic contract-month engine publishes those series. One user
symbol may appear in several pairs, for example against both the trade and
the mid of a market.

## Connectors

| `--feed` | Reads | Notes |
|---|---|---|
| `csv` | A delimited file or standard input (`--feed-path -`) | Columns below |
| `jsonl` | One JSON object per line | Same column flags; keys may be nested (`top.bid`) |
| `dbn` | A DBN file, `.dbn` or `.dbn.zst` (detected) | Trades, MBO trade and fill events, top of book (MBP-1, MBP-10 level 0, BBO, CMBP-1, CBBO) and OHLCV closes; prices in DBN fixed point; `--dbn-ts event` (default) or `recv` |
| `databento` | The vendor's live API with **your** key | Build with `cargo build -p mesh-client --features databento-live`. `--dataset <your dataset code>`, `--schema trades` (or `mbp-1`, `bbo-1s`, ...), key from the variable named by `--key-env` (default `DATABENTO_API_KEY`). It subscribes to the `user_symbol`s of your active pairs |

CSV and JSON lines columns:

| Flag | Default | |
|---|---|---|
| `--ts-col` | `ts` | Timestamp column (header name, key, or 0-based index with `--no-header`) |
| `--ts-format` | `ns` | `ns`, `us`, `ms`, `s` (fractions allowed) or `rfc3339` (`YYYY-MM-DDTHH:MM:SS[.f][Z]`, UTC) |
| `--symbol-col` | `symbol` | Symbol column |
| `--symbol` | none | One symbol for every row instead of a column |
| `--price-col` | `price` | Trade or last price column (not defaulted when `--bid-col` and `--ask-col` are given) |
| `--bid-col`, `--ask-col` | none | Top of book columns (both or neither) |
| `--delimiter` | `,` | CSV delimiter |
| `--no-header` | off | CSV has no header row |

Rows without any price are skipped. A malformed row stops the merge with its
line number; error messages never quote your values.

Your API key is read from the environment, used only to authenticate with
your provider, and never logged or written anywhere. Do not put it on the
command line.

## Outputs

| Flag | Default | |
|---|---|---|
| `--out` | `-` (standard output) | Merged side-by-side stream, JSON lines; a file you choose |
| `--no-merged` | off | Statistics only |
| `--stats` | standard error | Tracking statistics, JSON lines |
| `--stats-every` | end only | Also write statistics periodically while live |
| `--ws` | none | Serve merged and statistics lines on a local websocket, e.g. `127.0.0.1:9100`. Loopback addresses only; `0.0.0.0`, LAN addresses and host names are refused |
| `--ws-allow-origin` | none | Browser pages may connect only from loopback origins (`http://localhost:*`, `http://127.0.0.1:*`); add other origins explicitly. Non-browser clients send no origin and are accepted |

A merged line, one per pair per grid interval in which both sides have a
fresh price:

```json
{"provenance":"user_supplied","type":"sample","pair":"btc-perp-vs-mine","ts_ns":"1767225602000000000",
 "user":{"symbol":"MY-BTC","price":100.5,"ts_ns":"1767225601500000000"},
 "bleh":{"series":"hyperliquid:perp:BTC","price":100.62,"ts_ns":"1767225601900000000"},
 "basis":0.12,"basis_bps":11.94}
```

`multiplier` appears when it is not 1; `basis` is `bleh - user x multiplier`.

A statistics line, per pair:

```json
{"provenance":"user_supplied","type":"tracking","pair":"btc-perp-vs-mine","samples":3600,"interval_ms":1000,
 "first_ts_ns":"1767225600000000000","last_ts_ns":"1767229199000000000",
 "basis":{"mean":0.11,"std":0.04,"min":-0.02,"max":0.25,"last":0.1},"basis_bps":{"mean":10.9,"std":4.1},
 "returns":3590,"tracking_error_bps":1.8,"return_corr":0.97,
 "lead_lag":{"bleh_lead_ms":-1000,"corr":0.98,"corr_at_zero":0.95,"pairs":3580,"max_lag_ms":10000},
 "inputs":{"user":51234,"bleh":20480,"late":0,"stale":12,"dropped":0}}
```

## Alignment and statistics

- **Grid.** Each pair is sampled on a grid of `--interval` (default `1s`).
  A grid time is sampled once both sides have moved past it, using each
  side's last price at or before it. A price older than `--max-stale`
  (default `30s`) makes the interval stale (skipped and counted). An
  observation older than an interval already sampled is late (dropped and
  counted). bleh.xyz block heartbeats move the bleh.xyz side forward through
  quiet periods.
- **Clock.** `--clock event` (default) uses the feeds' own timestamps: your
  feed's event time (DBN `ts_event`, or `ts_recv` with `--dbn-ts recv`) and
  bleh.xyz venue time. Lead/lag then measures price discovery. `--clock
  receive` (live only) stamps both sides with local arrival time and samples
  every interval, so it measures what this machine sees, delivery latency
  included.
- **Basis**: `bleh - user x multiplier`, in price units and in basis points
  of your price.
- **Tracking error**: standard deviation of the difference of one-interval
  log returns, in basis points per interval. Returns need positive prices
  on both sides; intervals across a gap do not count.
- **Lead/lag**: over the most recent `--window` intervals (default 3600),
  the shift up to `--max-lag` (default `10s`) that maximizes the correlation
  of bleh.xyz returns with your returns that many intervals later.
  `bleh_lead_ms` positive means bleh.xyz moves first; negative means your
  feed moves first. Ties keep the smaller shift.

All of this is computed locally. If you choose to share results, that is
your decision under your license; bleh.xyz does not collect them.

## Refusals

| Refusal | Why |
|---|---|
| `refusing to write user-supplied data to ...: ... holds _manifests/ (a bleh.xyz archive)` | `--out` or `--stats` points inside a bleh.xyz archive (an ancestor directory holds `_manifests/`, `_iceberg/` or a signed bleh manifest, or is a `kind=`/`date=`/... partition directory), or the file is named like a manifest |
| `... is a bleh.xyz source checkout` | The output would land inside a clone of this repository |
| `refusing to bind ...: the local websocket binds a loopback address only` | `--ws` is not a literal loopback IP |
| `403` on websocket connect | A browser page from a non-loopback origin tried to connect |

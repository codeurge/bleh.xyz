# bleh.xyz launch copy

Status when written: early preview, in development, nothing live. Every piece
below says so. Do not post any of it with claims of a live feed, user counts,
latency or uptime until those are true and measured.

## X thread (8 posts)

**1/**
Market data? bleh.

We're building an open market-data network: futures-grade trades, order-level books, bars and order flow, built only from open price discovery.

Signed. Mirrored. Public domain. No permission slip.

Early preview. Nothing is live yet. Thread:

**2/**
The problem, in one sentence: prices are made in public, then somebody fences them off.

Take a licensed feed and you inherit the contract. So does everything you derive from it. Your backtest becomes someone's product.

**3/**
So we don't touch licensed feeds. At all.

The same economic exposure now trades on open, on-chain order books where every order is public. We read those from our own nodes, starting with every market on Hyperliquid, plus the swap trades US law already makes public.

**4/**
What that gets you (planned, in development):

• trades with native aggressor side
• order-by-order books, no embargo
• bars from 1s to 1d
• open footprint, delta, volume profile
• history back to each venue's first public data

**5/**
Don't trust us. Check.

Each block's records hang off a Merkle root signed with secp256k1. Daily manifests are hash-chained and timestamped on Bitcoin. Any relay or mirror can carry the data; you verify it yourself.

**6/**
How it stays up: there's nobody to send the letter to.

Nothing upstream is licensed. The data is CC0. The archive is content-addressed and goes out over CDN, BitTorrent and IPFS. Every copy is the original.

**7/**
Honest labels, because the category is short of them:

These are prices for the same exposure, not exchange prices or official settlements. Thin markets get called tapes, not benchmarks. Gaps are published as gaps.

**8/**
Code is Apache-2.0. Data is CC0. There is no token.

It's early and it's all in the open. Read the plan, poke holes in it, watch it get built:

github.com/codeurge/bleh.xyz

## Show HN

**Title**

Show HN: bleh.xyz – open, signed market data from on-chain order books (early preview)

**Text**

Hi HN. bleh.xyz is an open-source project to publish futures-grade market data that anyone can redistribute: trades with aggressor side, order-by-order books, bars, and order-flow analytics such as footprint, delta and volume profile.

Upfront: this is an early preview. The design, source review and plans are public, and the live and historical services are in development. There is nothing to connect to yet. I'm posting now because the design is the part I most want criticised.

The idea. Exchange prices are controlled through data contracts, and anything derived from a licensed feed inherits the contract. Rather than argue with that, the project never touches a licensed feed. It collects prices for the same economic exposure from places where it trades in the open: on-chain order-book venues read from our own nodes (first, every market on Hyperliquid, which now includes index, oil, metals, FX, rates and grain perpetuals), and the swap trades US regulation requires to be published for free. Arbitrage keeps those prices close to the exchange's. The link is economic, not contractual.

What is planned for the first releases:

- A websocket gateway with trades, order-level books, bars and analytics for every market on the first venue
- Per-block signing: each stream's records in a block are Merkle leaves, and the root is signed with secp256k1, so relays and mirrors need not be trusted
- A static Parquet archive back to the venue's earliest public data, queryable in place with DuckDB or Polars
- Distribution over CDN, BitTorrent and IPFS, with daily manifests timestamped on Bitcoin
- Python, TypeScript and Rust SDKs, a replay API, compatibility layers for existing clients, and an MCP server

What it is not: exchange prices, official settlements, or something to mark exchange positions against. Thin markets are labelled as tapes rather than benchmarks, and the coverage page lists the gaps (listed options, tick-level rates, agriculture depth, and too much weight on one venue for now).

Code is Apache-2.0, data is CC0, and there is no token.

Repo: https://github.com/codeurge/bleh.xyz

I'd most like feedback on the source admission policy, the signing scheme, and whether the "same exposure, open source" framing holds up for how you would use the data.

## Reddit post

Suggested for data-engineering, algorithmic-trading and open-data communities. Check each community's self-promotion rules first.

**Title**

We're building an open, CC0 alternative to licensed market data, sourced from on-chain order books. Early preview, feedback wanted

**Body**

Anyone who has priced a real-time futures feed knows the routine: per-user fees, extra for depth, no redistribution, and you lose your history when you stop paying.

bleh.xyz is an open-source attempt at a different route. It never touches an exchange-licensed feed. It records the same economic exposures where they trade in the open: on-chain order-book venues read from our own nodes (starting with every market on Hyperliquid, including index, oil, metals, FX, rates and grains), plus US public swap dissemination.

**Status: early preview. The design and plans are public; the services are in development and nothing is live yet.**

Planned for the first releases:

- Trades with native aggressor side and order-by-order books, no embargo
- Bars from 1 second to 1 day, and open order-flow analytics (footprint, delta, volume profile)
- Every block signed (secp256k1 Merkle roots), so you can verify data from any mirror
- A Parquet archive back to each venue's first public data, over CDN, BitTorrent and IPFS, anchored on Bitcoin
- Python, TypeScript and Rust SDKs, a replay API and an MCP server

Things I'd rather say now than have you find later: these are proxies, not exchange prices or settlements. Some markets are thin and will be labelled as tapes, not benchmarks. Listed options and tick-level rates are real gaps.

Code is Apache-2.0, data is CC0, no token.

Repo and docs: https://github.com/codeurge/bleh.xyz

What would you need from this before you'd backtest on it?

## GitHub repository

**Description** (under 350 characters)

Open market-data network. Futures-grade trades, order-level books, bars and order-flow analytics built only from open price discovery. Signed per block, mirrored over CDN, BitTorrent and IPFS. Code Apache-2.0, data CC0. Early preview.

**Website**

https://bleh.xyz

**Topics**

`market-data` `open-data` `order-book` `order-flow` `tick-data` `futures` `hyperliquid` `websocket` `parquet` `merkle-tree` `bittorrent` `ipfs` `cc0` `rust` `mcp`

## Boilerplate

bleh.xyz is an open-source market-data network that publishes futures-grade trades, order-level books, bars and order-flow analytics built only from open, independent price discovery. Every block is signed, the archive is mirrored across independent networks and anchored on Bitcoin, the code is Apache-2.0 and the data is public domain under CC0. (Status line to append while it applies: the project is in early preview and nothing is live yet.)

## Short lines

For cards, alt text and bios.

- Market data? bleh.
- Market data without the permission slip.
- Order books, no embargo.
- Signed. Mirrored. CC0.
- We don't do takedowns.
- A price is a fact. Facts are free.
- There is nobody to send the letter to.

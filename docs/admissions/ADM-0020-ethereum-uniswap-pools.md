# ADM-0020: Ethereum mainnet — Uniswap v3 and hookless Uniswap v4 pool swaps (ETH, wrapped BTC, tokenized gold)

- Status: Admitted
- Requested: 2026-11-14 by the research lane
  (open sources survey,
  `docs/research/open-sources-survey-2026-11.md`, rank 1)
- Reviewers: Repository owner (engineering review of field lineage)
- Decided: 2026-11-15 by the repository owner (owner decision, 2026-11-15)

## Source

Uniswap is an automated market maker on Ethereum mainnet. Each pool holds
two tokens, and its price moves only when someone swaps against the
liquidity that providers have deposited. The pool's own curve sets every
swap price, and no outside price is an input to the contract.

- **Uniswap v3** (from 2021): one contract per pool. There are no hooks.
- **Uniswap v4** (from early 2025): one `PoolManager` contract holds every
  pool. A pool may name a *hook* contract that runs before and after swaps.
  A hook can change how a swap is priced, for example by returning its own
  amounts, using an oracle or setting a dynamic fee. This record covers only
  v4 pools whose hook address is zero.

**How we read it.** Public chain state from an Ethereum node the project
runs on the home-archive server:

- an Erigon execution client (LGPL-3.0) with its built-in consensus client,
  synced from the peer-to-peer network;
- pre-Merge history (Uniswap v3 from 2021 to 2022-09) imported from era1
  files, which are raw chain history checked by the node against the chain.

**What it adds.** A second independent venue for ETH, wrapped BTC and gold,
trading at weekends and on holidays. See the survey's section "How extra
sources raise accuracy".

### Pools in scope

The reader handles only the pools listed in the registry. Each registry
entry records the following, taken from the factory's or the
`PoolManager`'s pool-creation event:

- the pool address (or the v4 pool ID);
- both tokens;
- the fee tier and tick spacing;
- for v4 pools, the hook address, which must be zero.

The first list, by exposure:

| Exposure | Pools |
|---|---|
| ETH | WETH/USDC and WETH/USDT, v3, at the 0.05% and 0.30% fee tiers; hookless v4 ETH/USDC and ETH/USDT pools |
| BTC (wrapped) | WBTC/USDC, WBTC/WETH, cbBTC/USDC, cbBTC/WETH, v3 |
| Gold (tokenized) | PAXG/USDC, PAXG/WETH, XAUT/USDT, PAXG/XAUT, v3 |

Adding a pool is a registry change and needs no new admission, as long as
the pool meets two conditions:

- it is a v3 pool or a hookless v4 pool;
- its tokens are listed in this record.

New tokens, other AMMs (Curve, Balancer and others) and other chains need an
amendment or a new record.

## Fields requested

| Field | Meaning | Admit? |
|---|---|---|
| v3 `Swap`: `amount0`, `amount1`, `sqrtPriceX96`, `liquidity`, `tick`; with block number, block time, transaction hash and log index | Signed token amounts of the swap, the pool price after it, active liquidity and tick. Execution price is the ratio of the amounts, adjusted for token decimals | Requested |
| v4 `Swap`: `id`, `amount0`, `amount1`, `sqrtPriceX96`, `liquidity`, `tick`, `fee`; with block number, block time, transaction hash and log index | Same, for a v4 pool, plus the fee applied | Requested, hookless pools only |
| v3 `Mint` and `Burn`: `tickLower`, `tickUpper`, `amount`, `amount0`, `amount1` | Liquidity added or removed over a price range. Used to rebuild pool depth for the liquidity grade | Requested |
| v4 `ModifyLiquidity`: `id`, `tickLower`, `tickUpper`, `liquidityDelta` | Same, for a v4 pool | Requested, hookless pools only |
| Pool metadata: tokens, decimals, fee tier, tick spacing, hook address; v3 `Initialize` and v4 `Initialize` price and tick | Registry facts | Requested |
| `sender`, `recipient`, `owner` (v3); `sender` (v4) | Account addresses | **Excluded.** Identify users, as owner addresses do in ADM-0001 |
| v3 `Collect`, `Flash`; v4 `Donate` | Fee collection, flash loans and donations | **Excluded.** Not market data |
| Pool observations (`observe`, the v3 time-weighted price accumulator) | Derived from the pool's own swaps | Not requested. Can be recomputed from admitted swaps |
| Any v4 pool with a non-zero hook address | Swap pricing may be set by the hook | **Excluded** until a specific hook is reviewed in an amendment |
| UniswapX fills, RFQ fills, aggregator quotes | Filled by off-chain market makers and settled on-chain, not formed in a pool | **Excluded** |
| Token prices from any oracle | Outside prices | **Excluded** |

## Policy checks

Under [admission-policy.md](../admission-policy.md):

1. **Original price formation.** Each swap's price comes from the pool's own
   curve and the liquidity deposited in it.
   - v3 pools and hookless v4 pools take no outside price.
   - Arbitrage carries information between venues, as it does on every
     venue, but no outside price is a field or a contract input.
   - Hooked v4 pools fail this test, because a hook can price swaps from an
     oracle; they are excluded.
2. **No contract on the path.** Data comes from our own node.
   - Erigon is LGPL-3.0.
   - era1 files are raw chain history mirrored without stated terms, and
     the node checks them against the chain, so no provider's data product
     is involved.
   - Public RPC endpoints are not used. For example, PublicNode's terms say
     the data may not be resold "as a standalone product, such as an RPC or
     data API service, without our prior written consent"
     (<https://www.publicnode.com/terms>).
   - The reader is written from the published event interfaces. No Uniswap
     code is copied.
3. **Venue-matched fields only.** Swap amounts and post-swap prices are the
   pool's own execution. Two things are recorded as facts, not hidden:
   - **Quote currency:** the pools quote in stablecoins (USDC, USDT) or
     WETH, not dollars. Records carry the quote token, and any conversion to
     dollars belongs to a method, not to this record.
   - **Issuer claims:** WBTC, cbBTC, PAXG and XAUT are claims on custodians
     or issuers, who can freeze addresses. The registry states each token's
     issuer and backing as a fact.
4. **Field-level exclusions.** As listed above: addresses, hooked pools,
   fills not formed in a pool, fee and flash events, oracle prices.

## Source basis

Read 2026-11-14:

- Uniswap v3 pool events:
  <https://github.com/Uniswap/v3-core/blob/main/contracts/interfaces/pool/IUniswapV3PoolEvents.sol>
- Uniswap v4 `PoolManager` events (the `Initialize` event carries the hook
  address; `Swap` carries the fee):
  <https://github.com/Uniswap/v4-core/blob/main/src/interfaces/IPoolManager.sol>
- Erigon hardware requirements, for mainnet:
  - full node: 920 GB required, 2 TB recommended, 16–32 GB RAM;
  - archive node: 1.77 TB required, 4 TB recommended.

  Source: <https://erigon-documentation.pages.dev/getting-started/hw-requirements>.
- Reth pruning: a `--full` node keeps receipts only for the last 10,064
  blocks, so it cannot serve old logs
  (<https://reth.rs/run/storage/pruning>). Whichever client is used must
  keep every receipt. Erigon is reported to do so by default from v3.7; this
  is to verify at install.
- Pre-Merge history endpoints (era1, with checksums):
  <https://eth-clients.github.io/history-endpoints/>

## Infrastructure and cost

- **Where:** the home-archive server. It has enough RAM. Its disk array is
  HDD, which is too slow for an Ethereum node, and its 500 GB scratch disk
  is too small.
- **What is needed:** a dedicated 2 TB SSD or NVMe drive for a full node
  that keeps receipts. **Cost:** $0 if a drive is free, otherwise roughly
  $120–200 one-off (estimate, not a quote).
- **No recurring cost** beyond home electricity and bandwidth. Initial sync
  downloads on the order of 1 TB.

## Open points before ingest

- **Liquidity grade.** An AMM pool has no order book. The book and
  analytics lane has to define how pool depth (from tick liquidity) and the
  fee tier map to the grade inputs. Until then, pools are ungraded and count
  as `tape` in [synthetic-months@1](../specs/method-synthetic-months-v1.md)
  § 10.
- **Finality.** Blocks can reorganize until they are finalized, about two
  epochs later. Choose one of two approaches:
  - publish records only from finalized blocks; or
  - publish head records with a retraction path.
- **Stablecoin quotes.** Composite methods that mix these pools with
  dollar-quoted sources need an explicit stablecoin assumption, recorded in
  a method version.

## Risks and review triggers

- A listed pool turns out to have pricing set outside its curve (for
  example, a v4 pool migrates to a hooked pool with the same tokens).
- A token issuer freezes pool balances, or a token loses its peg or
  backing. Prices from that pool stop tracking the exposure, and the
  registry marks the pool.
- Uniswap governance changes how pool fees work in a way that alters swap
  prices.
- The node client changes its receipt retention, or era1 mirrors disappear.
  Other mirrors and the torrent listed on the history-endpoints page are
  the fallback.

## Decision

Admitted on 2026-11-15 by the repository owner for the fields and pools above,
read only from the project's own Ethereum node as described in "How we
read it". No public RPC endpoint or data provider is used. Ingestion
starts when the node has its dedicated SSD (see "Where").

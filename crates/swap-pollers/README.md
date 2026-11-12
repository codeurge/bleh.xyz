# swap-pollers

Pollers for CFTC Part 43 and SEC SBSR public swap dissemination, normalized
to `SwapPrint` records. Owned by the swap-pollers lane; see
`docs/decisions/0007-code-layout.md`.

Status: implemented for DTCC Data Repository (U.S.) public price
dissemination, CFTC and SEC buckets (ADM-0002, ADM-0004). Repositories
operated by an exchange group, such as ICE Trade Vault, are excluded by
those admissions.

Every disseminated message becomes a record: all six actions (`new`,
`modify`, `terminate`, `correct`, `cancel`, `revive`) and non-trade `new`
messages (clearing, allocation, ...) with `event_type` set, so consumers
counting prints or volume select `event_type` `trade` or absent. CFTC
records also carry option terms (`strike`, `option_premium`, `option_type`,
`option_style`, `first_exercise_date`); SEC records do not, because
ADM-0004 admits price, spread and notional only. Monetary spreads are
`spread_amount`, and commodity prints with no currency notional carry
`quantity`.

- Formats and mapping: `docs/specs/swap-repositories.md` (§ 4)
- Running, configuration and metrics: `docs/operations/swap-pollers.md`

Library entry point for an embedding gateway:
`swap_pollers::config::run(&args, Some(sender), shutdown)` sends every
emitted `SwapPrint` on the channel as well as writing daily JSON lines.

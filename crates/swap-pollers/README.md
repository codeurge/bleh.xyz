# swap-pollers

Pollers for CFTC Part 43 and SEC SBSR public swap dissemination, normalized
to `SwapPrint` records. Owned by the swap-pollers lane; see
`docs/decisions/0007-code-layout.md`.

Status: implemented for DTCC Data Repository (U.S.) public price
dissemination, CFTC and SEC buckets (ADM-0002, ADM-0004). Repositories
operated by an exchange group, such as ICE Trade Vault, are excluded by
those admissions.

- Formats and mapping: `docs/specs/swap-repositories.md`
- Running, configuration and metrics: `docs/operations/swap-pollers.md`

Library entry point for an embedding gateway:
`swap_pollers::config::run(&args, Some(sender), shutdown)` sends every
emitted `SwapPrint` on the channel as well as writing daily JSON lines.

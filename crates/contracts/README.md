# contracts

Solidity verifier for bleh.xyz block commitments, tested against the Rust
signer in an in-process EVM.

**Status:** implemented and tested. Test contract only: not deployed to any
chain, and it holds no keys.

## Responsibility

[`sol/BlehBlockVerifier.sol`](sol/BlehBlockVerifier.sol) verifies, with
`ecrecover`, exactly what the origin signs (`mesh_types::signing`,
`mesh_types::merkle`; observation-v0 § 7):

- `BlehCommitments`, an internal library a consumer contract can link in:
  `leafHash`, `nodeHash`, `commitmentHash`, `recoverSigner`,
  `verifyCommitment`, `verifyInclusion` and `verifyRecord`.
- `BlehBlockVerifier`, a stateless contract exposing the library.

Runbook and trust model: [docs/operations/bus.md](../../docs/operations/bus.md)
§ On-chain verification.

## Layout

| Path | Contents |
|---|---|
| `sol/BlehBlockVerifier.sol` | The Solidity source |
| `package.json`, `package-lock.json` | The pinned compiler (`solc` 0.8.38, solc-js) |
| `compile.mjs` | Compiles with pinned settings and writes `src/bytecode.rs` |
| `src/bytecode.rs` | Generated: bytecode, compiler version and settings, source SHA-256, selectors |
| `tests/evm.rs` | Deploys the bytecode in `revm` and checks it against the bus signer |

## Building

Rust tests need no Node.js: they use the committed bytecode.

```bash
cargo test -p contracts
```

After changing the Solidity source, regenerate the bytecode with the pinned
compiler (Node.js 20 or later; no install scripts run):

```bash
cd crates/contracts && npm ci --ignore-scripts && node compile.mjs
```

`node compile.mjs --check` fails if `src/bytecode.rs` differs from a fresh
build; CI runs it. `cargo test` also fails if the source changed without a
rebuild (the source SHA-256 is recorded in `src/bytecode.rs`).

Compiler settings: `evmVersion` `cancun`, optimizer on with 200 runs, no
IR pipeline, and no CBOR metadata, so the bytecode depends on the code only,
not on comments or paths.

## Rules

- Owned by the integrator with `crates/mesh-types`: a change to the signing
  scheme changes both, in one pull request with a decision record.
- Test keys only. No deployment to mainnet or testnets from this repository.
- See [ADR 0007](../../docs/decisions/0007-code-layout.md).

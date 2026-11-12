# hl-synth

Deterministic generator of **synthetic** Hyperliquid node-format files.

**Status:** implemented. **Owner:** integrator.

## Responsibility

Writes the golden fixture set under
[`fixtures/synthetic/hl/`](../../fixtures/synthetic/hl/) from a seeded
simulation, following
[hyperliquid-node-format.md](../../docs/specs/hyperliquid-node-format.md),
with an `expected.json` of what a correct reader and book builder produce.
No real market data is used.

```bash
cargo run -p hl-synth                       # regenerate (seed 42, 60 blocks)
cargo run -p hl-synth -- --check            # verify the committed files are current
cargo run -p hl-synth -- --seed 7 --out /tmp/hl   # another run elsewhere
```

`cargo test -p hl-synth` fails when the committed fixtures are stale.

## Rules

- Output depends only on the seed and this code (its own SplitMix64 RNG).
- When the node-format spec changes, update the generator, regenerate, and
  commit both together.

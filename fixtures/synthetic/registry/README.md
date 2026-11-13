# Example registry (registry-v0)

[registry-v0.json](registry-v0.json) is an example registry document
([registry-v0](../../../docs/specs/registry-v0.md)), validated against
[registry-v0.schema.json](../../../docs/specs/registry-v0.schema.json) and
parsed by `mesh_types::Registry` in the `mesh-types` tests.

**The markets, mappings and grades are synthetic.** The instruments are
the fictional markets of [../hl](../hl) (`SYN`, `syn:IDX`, `#9001`); the
venue mappings and liquidity grades that refer to them are invented. No
value comes from Hyperliquid, an exchange or any real market.

The calendars, holiday rule names, contract cycles, last-trading-day rules
and roll rules are public facts written fresh for this file, the same
facts as the synthetic-months example registry
([method-synthetic-months-v1](../../../docs/specs/method-synthetic-months-v1.md)
§§ 5–6). Exposure names are neutral; exchange contracts appear only as the
economic reference in `tracks`.

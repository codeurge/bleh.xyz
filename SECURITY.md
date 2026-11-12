# Security and data-integrity policy

## What to report privately

- Vulnerabilities in code, nodes, gateways or the archive.
- Signing-key compromise or forged observations.
- **Contamination:** evidence that an admitted source, a contributor or an
  output contains exchange-licensed data or data derived from it. Treat this
  as a security issue. Report it privately and do not open a public issue.

## How to report

Use the repository host's private vulnerability reporting once the
repository is public. Until then, contact the maintainers directly. A
dedicated address is tracked in
[.agents/plans/phase-0-foundation.md](.agents/plans/phase-0-foundation.md).

## What happens next

1. Acknowledgement within 3 business days.
2. For contamination: affected live outputs are withdrawn at once, under
   [docs/admission-policy.md](docs/admission-policy.md) § Removal, and the
   affected archive partitions are rebuilt as new versions with a new
   manifest.
3. For a signing-key compromise: the key is revoked in the registry, a new
   key is published, and manifests signed after the compromise are re-signed.
4. A fix or rebuild, then a public note in `CHANGELOG.md` once it is safe to
   disclose.

## Supported versions

There are no releases yet. This policy applies to `main`.

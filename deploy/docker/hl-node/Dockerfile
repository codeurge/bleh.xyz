# syntax=docker/dockerfile:1.29@sha256:572cebbc2b15e6f84c077742952674889cb63cd5e9b80abdffcbe587b6f19183
#
# Non-validating Hyperliquid node, installed with the official hl-visor
# procedure from https://github.com/hyperliquid-dex/node (Apache-2.0):
# visor.json names the chain, hl-visor is downloaded from the official
# binaries host and its detached signature is verified against the
# Hyperliquid release key, whose fingerprint is pinned here. hl-visor then
# downloads, verifies and runs hl-node itself (and upgrades it without
# notice; see docs/operations/node.md for schema-drift detection).
#
# Written from the README's documented procedure; no code is copied from the
# node repository or any unlicensed repository.
#
# Build:   deploy/docker/build.sh hl-node
# Run:     see deploy/compose/compose.yaml (profile "node") or
#          deploy/compose/prod/node-host.yaml. Host networking is required:
#          gossip uses TCP 4001-4002 and the info server listens on
#          localhost:3001.

ARG UBUNTU_IMAGE=docker.io/library/ubuntu:noble-20261110@sha256:c1c5d8818eca0a4fd0c6c187d459a7af264e7fbf8561bde85e93be2a3bbc8f4a

FROM ${UBUNTU_IMAGE}

# Mainnet or Testnet.
ARG HL_CHAIN=Mainnet
# The release key, fetched from a pinned commit of the node repository, and
# its expected OpenPGP fingerprint. The build fails if either changes. Check
# the fingerprint out of band before trusting a new value.
ARG HL_PUBKEY_URL=https://raw.githubusercontent.com/hyperliquid-dex/node/405cc08b17a727ee51b0f9128918955a84439915/pub_key.asc
ARG HL_PUBKEY_FINGERPRINT=CF2C2EA3DC3E8F042A55FB6503254A9349F1820B
# Ubuntu snapshot (YYYYMMDDTHHMMSSZ) for reproducible apt installs; empty
# uses the live archive.
ARG UBUNTU_SNAPSHOT=
ARG VCS_REF=unknown
# Matches the official image's user and UID so host volumes line up.
ARG HL_UID=10000

LABEL org.opencontainers.image.title="bleh hl-node (non-validator)" \
      org.opencontainers.image.source="https://github.com/codeurge/bleh.xyz" \
      org.opencontainers.image.revision="${VCS_REF}" \
      org.opencontainers.image.licenses="Apache-2.0"

SHELL ["/bin/bash", "-euo", "pipefail", "-c"]

RUN snap=(); \
    if [[ -n "${UBUNTU_SNAPSHOT}" ]]; then snap=(--snapshot "${UBUNTU_SNAPSHOT}"); fi; \
    apt-get "${snap[@]}" update; \
    DEBIAN_FRONTEND=noninteractive apt-get "${snap[@]}" install -y --no-install-recommends \
      ca-certificates curl gnupg jq tini; \
    rm -rf /var/lib/apt/lists/*; \
    groupadd --gid "${HL_UID}" hluser; \
    useradd --uid "${HL_UID}" --gid "${HL_UID}" --create-home --shell /usr/sbin/nologin hluser; \
    install -d -o hluser -g hluser /home/hluser/hl /home/hluser/hl/data /snapshots

USER hluser
WORKDIR /home/hluser

RUN case "${HL_CHAIN}" in \
      Mainnet) base="https://binaries.hyperliquid.xyz/Mainnet" ;; \
      Testnet) base="https://binaries.hyperliquid-testnet.xyz/Testnet" ;; \
      *) echo "HL_CHAIN must be Mainnet or Testnet" >&2; exit 1 ;; \
    esac; \
    curl -fsSL --retry 5 "${HL_PUBKEY_URL}" -o /tmp/pub_key.asc; \
    got=$(gpg --batch --with-colons --show-keys /tmp/pub_key.asc | awk -F: '/^fpr:/ {print $10; exit}'); \
    if [[ "${got}" != "${HL_PUBKEY_FINGERPRINT}" ]]; then \
      echo "release key fingerprint ${got} does not match ${HL_PUBKEY_FINGERPRINT}" >&2; exit 1; \
    fi; \
    gpg --batch --import /tmp/pub_key.asc; \
    printf '%s:6:\n' "${HL_PUBKEY_FINGERPRINT}" | gpg --batch --import-ownertrust; \
    curl -fsSL --retry 5 "${base}/hl-visor" -o hl-visor; \
    curl -fsSL --retry 5 "${base}/hl-visor.asc" -o hl-visor.asc; \
    gpg --batch --verify hl-visor.asc hl-visor; \
    chmod 0755 hl-visor; \
    printf '{"chain": "%s"}\n' "${HL_CHAIN}" > visor.json; \
    rm -f /tmp/pub_key.asc

COPY --chown=root:root --chmod=0755 deploy/docker/hl-node/entrypoint.sh /usr/local/bin/hl-node-entrypoint

ENV HL_CHAIN=${HL_CHAIN}

# Gossip. The info server (3001) stays on localhost and is not exposed.
EXPOSE 4001/tcp 4002/tcp

# Node output, hl-node binaries and state live here; mount a volume on fast
# NVMe. /snapshots receives fileSnapshot output for the reader.
VOLUME ["/home/hluser/hl", "/snapshots"]

ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/hl-node-entrypoint"]

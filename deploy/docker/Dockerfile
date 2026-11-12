# syntax=docker/dockerfile:1.29@sha256:572cebbc2b15e6f84c077742952674889cb63cd5e9b80abdffcbe587b6f19183
#
# One multi-stage build for every bleh.xyz service binary. Each service is a
# separate target with a distroless, non-root final image (no shell, no
# package manager):
#
#   hl-node-reader  gateway  archive  history-api  swap-pollers  hl-backfill
#   mesh-client     tools (mesh-client + hl-synth)
#
# Build through deploy/docker/build.sh, which feeds `git archive` of a commit
# as the build context (so only committed files are built, and target/ and
# .git never enter the context) and tags images by git SHA:
#
#   deploy/docker/build.sh                 # every target, tagged with HEAD's SHA
#   deploy/docker/build.sh gateway archive # selected targets
#
# Reproducibility: base images are pinned by digest, Debian packages come
# from snapshot.debian.org at a fixed date, the toolchain comes from
# rust-toolchain.toml, dependencies from Cargo.lock (--locked), build paths
# are remapped and SOURCE_DATE_EPOCH is the commit time. The pinned build
# container is the byte-identity boundary named in ADR 0006.

ARG RUST_IMAGE=docker.io/library/rust:1.99.1-slim-trixie@sha256:f6c4a35d21dbb3c89d7a8d056c193e94fc09037f2b58dd858b09c39a47dd02d7
ARG RUNTIME_IMAGE=gcr.io/distroless/cc-debian13:nonroot@sha256:cc66d222f0a7ec996b78a99fe76a969bc80b539a5016ad866c14d1949bc6d0ab

# ---------------------------------------------------------------------------
# Build stage: compiles every binary in the workspace once.
# ---------------------------------------------------------------------------
FROM ${RUST_IMAGE} AS build

# Debian snapshot date for build dependencies. Set to an empty string to use
# the live mirrors (faster, not reproducible).
ARG DEBIAN_SNAPSHOT=20261110T000000Z
ARG SOURCE_DATE_EPOCH=0
ARG BINS="hl-node-reader gateway archive history-api swap-pollers hl-backfill mesh-client hl-synth"

ENV CARGO_INCREMENTAL=0 \
    CARGO_TERM_COLOR=never \
    CARGO_NET_RETRY=5 \
    SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH} \
    RUSTFLAGS="--remap-path-prefix=/src=/bleh --remap-path-prefix=/usr/local/cargo=/cargo --remap-path-prefix=/usr/local/rustup=/rustup"

# g++ and make: C/C++ sources in dependencies (zstd, DuckDB bundled build).
# pkg-config: crates that probe for system libraries.
RUN set -eux; \
    if [ -n "${DEBIAN_SNAPSHOT}" ]; then \
      printf '%s\n' \
        'Types: deb' \
        "URIs: http://snapshot.debian.org/archive/debian/${DEBIAN_SNAPSHOT}" \
        'Suites: trixie trixie-updates' \
        'Components: main' \
        'Signed-By: /usr/share/keyrings/debian-archive-keyring.gpg' \
        'Check-Valid-Until: no' \
        '' \
        'Types: deb' \
        "URIs: http://snapshot.debian.org/archive/debian-security/${DEBIAN_SNAPSHOT}" \
        'Suites: trixie-security' \
        'Components: main' \
        'Signed-By: /usr/share/keyrings/debian-archive-keyring.gpg' \
        'Check-Valid-Until: no' \
        > /etc/apt/sources.list.d/debian.sources; \
    fi; \
    apt-get update; \
    apt-get install -y --no-install-recommends g++ make pkg-config; \
    rm -rf /var/lib/apt/lists/*

WORKDIR /src

# Install the pinned toolchain in its own layer so it is cached across
# source changes.
COPY rust-toolchain.toml ./
RUN rustup toolchain install && rustup show active-toolchain

COPY . .

RUN --mount=type=cache,id=bleh-cargo-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=bleh-cargo-git,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,id=bleh-target,target=/src/target,sharing=locked \
    set -eux; \
    cargo build --locked --release --workspace --bins; \
    mkdir -p /out; \
    for b in ${BINS}; do install -m 0755 "target/release/${b}" "/out/${b}"; done; \
    (cd /out && sha256sum * > SHA256SUMS); \
    cat /out/SHA256SUMS; \
    mkdir -p /state/raw /state/archive /state/backfill

# ---------------------------------------------------------------------------
# Runtime base: distroless glibc + libgcc + libstdc++, user 65532 (nonroot).
# ---------------------------------------------------------------------------
FROM ${RUNTIME_IMAGE} AS runtime
ARG VCS_REF=unknown
LABEL org.opencontainers.image.source="https://github.com/codeurge/bleh.xyz" \
      org.opencontainers.image.revision="${VCS_REF}" \
      org.opencontainers.image.licenses="Apache-2.0" \
      org.opencontainers.image.vendor="bleh.xyz"
ENV RUST_LOG=info \
    RUST_BACKTRACE=1
USER nonroot:nonroot
COPY --from=build /out/SHA256SUMS /usr/share/bleh/SHA256SUMS
# State directories owned by the runtime user, so a named volume mounted
# here starts writable. Bind mounts on hosts are chowned to 65532 by
# deploy/scripts/host-bootstrap.sh.
COPY --from=build --chown=65532:65532 /state/ /var/lib/bleh/

# ---------------------------------------------------------------------------
# Service targets. Ports are the defaults listed in deploy/README.md
# ("Service contract"); every service also takes its listen addresses from
# the environment.
# ---------------------------------------------------------------------------
FROM runtime AS hl-node-reader
LABEL org.opencontainers.image.title="bleh hl-node-reader"
COPY --from=build /out/hl-node-reader /usr/local/bin/hl-node-reader
EXPOSE 7400 9101
ENTRYPOINT ["/usr/local/bin/hl-node-reader"]

FROM runtime AS gateway
LABEL org.opencontainers.image.title="bleh gateway"
COPY --from=build /out/gateway /usr/local/bin/gateway
EXPOSE 8080 9102
ENTRYPOINT ["/usr/local/bin/gateway"]

FROM runtime AS archive
LABEL org.opencontainers.image.title="bleh archive"
COPY --from=build /out/archive /usr/local/bin/archive
EXPOSE 9103
ENTRYPOINT ["/usr/local/bin/archive"]

FROM runtime AS history-api
LABEL org.opencontainers.image.title="bleh history-api"
COPY --from=build /out/history-api /usr/local/bin/history-api
EXPOSE 8081 9104
ENTRYPOINT ["/usr/local/bin/history-api"]

FROM runtime AS swap-pollers
LABEL org.opencontainers.image.title="bleh swap-pollers"
COPY --from=build /out/swap-pollers /usr/local/bin/swap-pollers
EXPOSE 9105
ENTRYPOINT ["/usr/local/bin/swap-pollers"]

FROM runtime AS hl-backfill
LABEL org.opencontainers.image.title="bleh hl-backfill"
COPY --from=build /out/hl-backfill /usr/local/bin/hl-backfill
EXPOSE 9106
ENTRYPOINT ["/usr/local/bin/hl-backfill"]

FROM runtime AS tools
LABEL org.opencontainers.image.title="bleh tools"
COPY --from=build /out/mesh-client /usr/local/bin/mesh-client
COPY --from=build /out/hl-synth /usr/local/bin/hl-synth
ENTRYPOINT ["/usr/local/bin/mesh-client"]

FROM runtime AS mesh-client
LABEL org.opencontainers.image.title="bleh mesh-client"
COPY --from=build /out/mesh-client /usr/local/bin/mesh-client
ENTRYPOINT ["/usr/local/bin/mesh-client"]

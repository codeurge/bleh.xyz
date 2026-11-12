#!/usr/bin/env bash
# Native install of hl-visor for deploy/systemd/hl-visor.service (the
# alternative to the hl-node container). Follows the documented procedure of
# https://github.com/hyperliquid-dex/node: download hl-visor from the
# official binaries host and verify its signature against the Hyperliquid
# release key, whose fingerprint is pinned (same values as
# deploy/docker/hl-node/Dockerfile; change both together).
#
#   sudo deploy/scripts/node-install.sh [Mainnet|Testnet]
set -euo pipefail

chain=${1:-Mainnet}
pubkey_url=https://raw.githubusercontent.com/hyperliquid-dex/node/405cc08b17a727ee51b0f9128918955a84439915/pub_key.asc
fingerprint=CF2C2EA3DC3E8F042A55FB6503254A9349F1820B
home=/srv/hl

[[ $EUID -eq 0 ]] || { echo "run as root" >&2; exit 77; }
id hl >/dev/null 2>&1 || { echo "user hl missing: run host-bootstrap.sh node-box first" >&2; exit 1; }
case "$chain" in
  Mainnet) base=https://binaries.hyperliquid.xyz/Mainnet ;;
  Testnet) base=https://binaries.hyperliquid-testnet.xyz/Testnet ;;
  *) echo "chain must be Mainnet or Testnet" >&2; exit 64 ;;
esac

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
export GNUPGHOME="$home/.gnupg"
install -d -o hl -g hl -m 0700 "$GNUPGHOME"

curl -fsSL --retry 5 "$pubkey_url" -o "$work/pub_key.asc"
got=$(gpg --batch --with-colons --show-keys "$work/pub_key.asc" | awk -F: '/^fpr:/ {print $10; exit}')
[[ "$got" == "$fingerprint" ]] || { echo "release key fingerprint $got != $fingerprint" >&2; exit 1; }
gpg --batch --import "$work/pub_key.asc"
printf '%s:6:\n' "$fingerprint" | gpg --batch --import-ownertrust

curl -fsSL --retry 5 "$base/hl-visor" -o "$work/hl-visor"
curl -fsSL --retry 5 "$base/hl-visor.asc" -o "$work/hl-visor.asc"
gpg --batch --verify "$work/hl-visor.asc" "$work/hl-visor"

install -o hl -g hl -m 0755 "$work/hl-visor" "$home/hl-visor"
printf '{"chain": "%s"}\n' "$chain" > "$home/visor.json"
chown -R hl:hl "$GNUPGHOME" "$home/visor.json"
[[ -f /etc/bleh/hl-visor.env ]] || install -m 0644 /opt/bleh/src/deploy/systemd/env/hl-visor.example.conf /etc/bleh/hl-visor.env
install -m 0644 /opt/bleh/src/deploy/systemd/hl-visor.service /etc/systemd/system/hl-visor.service
systemctl daemon-reload
echo "installed $home/hl-visor (verified). Start with: systemctl enable --now hl-visor"
echo "Do not also run the hl-node container."

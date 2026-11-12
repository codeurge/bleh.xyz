#!/usr/bin/env bash
# Prepare an Ubuntu 24.04 host for a bleh.xyz role. Idempotent: safe to run
# again after changing options or pulling a new commit. cloud-init runs it
# on rented hosts (deploy/terraform/modules/host); on a self-hosted machine
# or a manually ordered dedicated server, run it by hand from a checkout:
#
#   sudo deploy/scripts/host-bootstrap.sh <role> [options]
#
# Roles:
#   node-box      Hyperliquid node box (node, reader, gateway, capture)
#   home-archive  archive, seed and processing server
#   relay         public relay VM
#   origin        node-box and home-archive on one machine
#
# Options:
#   --site NAME         label for metrics and logs (a, b, home, relay-1, ...)
#   --overlay-ip IP     WireGuard overlay address (deploy/README.md, "Overlay")
#   --ssh-cidr CIDR     allow SSH from CIDR (repeatable; default: anywhere)
#   --node-disk MODE    auto (default): build /srv/hl from unused NVMe disks;
#                       none: use the root filesystem; or a block device
#   --no-firewall       leave the host firewall alone
#   --auto-reboot       reboot for security updates at 04:30 UTC (relays)
#
# What it does: base packages, Docker (Ubuntu packages, journald-friendly
# log limits, live-restore), chrony with NTS sources, unattended security
# updates, sysctls for inotify and many sockets, journald caps, the data
# directories and their owners, the node data disk, an nftables input
# policy per role, a WireGuard key, systemd timers (pruning, schema drift,
# probe, host metrics) and deploy/compose/.env values for the role.
set -euo pipefail

role=${1:-}
[[ -n "$role" ]] || { sed -n '2,30p' "$0" | sed 's/^# \{0,1\}//'; exit 64; }
shift
site=""
overlay_ip=""
ssh_cidrs=()
node_disk=auto
firewall=1
auto_reboot=0
while (($#)); do
  case "$1" in
    --site) site=${2:?}; shift 2 ;;
    --overlay-ip) overlay_ip=${2:?}; shift 2 ;;
    --ssh-cidr) ssh_cidrs+=("${2:?}"); shift 2 ;;
    --node-disk) node_disk=${2:?}; shift 2 ;;
    --no-firewall) firewall=0; shift ;;
    --auto-reboot) auto_reboot=1; shift ;;
    *) echo "unknown option $1" >&2; exit 64 ;;
  esac
done

case "$role" in node-box|home-archive|relay|origin) ;; *) echo "unknown role $role" >&2; exit 64 ;; esac
has_node=0; has_home=0
[[ "$role" == node-box || "$role" == origin ]] && has_node=1
[[ "$role" == home-archive || "$role" == origin ]] && has_home=1

[[ $EUID -eq 0 ]] || { echo "run as root (sudo)" >&2; exit 77; }
repo=$(cd "$(dirname "$0")/../.." && pwd)
log() { printf '\n== %s\n' "$*"; }

. /etc/os-release
if [[ "${ID:-}" != ubuntu || "${VERSION_ID:-}" != 24.04 ]]; then
  if ((has_node)); then
    echo "the Hyperliquid node supports Ubuntu 24.04 only (found ${PRETTY_NAME:-unknown}); use an Ubuntu 24.04 VM" >&2
    exit 1
  fi
  echo "warning: tested on Ubuntu 24.04 only (found ${PRETTY_NAME:-unknown})" >&2
fi

# ---------------------------------------------------------------- packages
log "packages"
export DEBIAN_FRONTEND=noninteractive
apt-get update -q
apt-get install -y -q --no-install-recommends \
  ca-certificates curl git jq gnupg chrony nftables wireguard-tools \
  unattended-upgrades apt-listchanges docker.io docker-compose-v2 \
  xfsprogs mdadm zstd smartmontools
timedatectl set-timezone UTC

# ---------------------------------------------------------------- repo path
# Units and timers refer to /opt/bleh/src.
mkdir -p /opt/bleh
if [[ "$repo" != /opt/bleh/src ]]; then ln -sfn "$repo" /opt/bleh/src; fi

# ---------------------------------------------------------------- time
log "chrony (alert above 5 ms offset)"
cat > /etc/chrony/conf.d/90-bleh.conf <<'EOF'
# NTS-authenticated public sources; add a provider-local server in
# /etc/chrony/conf.d/91-local.conf (for example 169.254.169.123 on AWS).
server time.cloudflare.com iburst nts
server nts.netnod.se iburst nts
server ptbtime1.ptb.de iburst nts
pool ntp.ubuntu.com iburst maxsources 2
makestep 0.1 3
rtcsync
leapsectz right/UTC
EOF
systemctl restart chrony

# ---------------------------------------------------------------- kernel
log "sysctl and limits"
cat > /etc/sysctl.d/90-bleh.conf <<'EOF'
# The reader tails many hourly files with inotify.
fs.inotify.max_user_watches = 1048576
fs.inotify.max_user_instances = 8192
fs.file-max = 4194304
# Gateway and relays hold many websocket connections.
net.core.somaxconn = 65535
net.ipv4.tcp_max_syn_backlog = 65535
net.ipv4.ip_local_port_range = 10240 65535
net.core.rmem_max = 16777216
net.core.wmem_max = 16777216
net.ipv4.tcp_keepalive_time = 60
net.ipv4.tcp_keepalive_intvl = 10
net.ipv4.tcp_keepalive_probes = 6
vm.swappiness = 10
EOF
sysctl --system >/dev/null

mkdir -p /etc/systemd/journald.conf.d
cat > /etc/systemd/journald.conf.d/90-bleh.conf <<'EOF'
[Journal]
SystemMaxUse=4G
MaxRetentionSec=30day
EOF
systemctl restart systemd-journald

# ---------------------------------------------------------------- updates
log "unattended security updates"
reboot_flag=false
((auto_reboot)) && reboot_flag=true
cat > /etc/apt/apt.conf.d/52bleh-unattended <<EOF
Unattended-Upgrade::Allowed-Origins {
  "\${distro_id}:\${distro_codename}-security";
  "\${distro_id}ESMApps:\${distro_codename}-apps-security";
  "\${distro_id}ESM:\${distro_codename}-infra-security";
};
Unattended-Upgrade::Remove-Unused-Dependencies "true";
// Node boxes reboot by hand, one origin at a time (docs/operations/upgrades.md).
Unattended-Upgrade::Automatic-Reboot "${reboot_flag}";
Unattended-Upgrade::Automatic-Reboot-Time "04:30";
EOF
cat > /etc/apt/apt.conf.d/20auto-upgrades <<'EOF'
APT::Periodic::Update-Package-Lists "1";
APT::Periodic::Unattended-Upgrade "1";
EOF

# ---------------------------------------------------------------- docker
log "docker"
mkdir -p /etc/docker /etc/systemd/system/docker.service.d
cat > /etc/docker/daemon.json <<'EOF'
{
  "live-restore": true,
  "log-driver": "json-file",
  "log-opts": {"max-size": "50m", "max-file": "5"},
  "default-ulimits": {"nofile": {"Name": "nofile", "Soft": 1048576, "Hard": 1048576}},
  "userland-proxy": false
}
EOF
# Ports published on the overlay address need the WireGuard interface first.
install -m 0644 "$repo/deploy/systemd/docker-after-wireguard.conf" \
  /etc/systemd/system/docker.service.d/10-bleh-wireguard.conf
systemctl daemon-reload
systemctl enable --now docker
systemctl restart docker

# ---------------------------------------------------------------- disks and directories
log "directories"
install -d -m 0755 /var/lib/node_exporter/textfile /var/lib/bleh /etc/bleh

if ((has_node)); then
  id hl >/dev/null 2>&1 || useradd --uid 10000 --user-group --home-dir /srv/hl --shell /usr/sbin/nologin hl
  if ! mountpoint -q /srv/hl && [[ "$node_disk" != none ]]; then
    "$repo/deploy/scripts/disk-setup.sh" /srv/hl "$node_disk" || echo "disk setup skipped; /srv/hl stays on the root filesystem" >&2
  fi
  install -d -o 10000 -g 10000 -m 0755 /srv/hl /srv/hl/hl /srv/hl/hl/data /srv/hl/snapshots
  install -d -o 65532 -g 65532 -m 0750 /srv/bleh /srv/bleh/raw /srv/bleh/reader /srv/bleh/archive
  # The origin (container user 65532) writes its capture counters into the
  # textfile collector directory (write to .prom.tmp, then rename).
  chgrp 65532 /var/lib/node_exporter/textfile
  chmod 0775 /var/lib/node_exporter/textfile
fi
if ((has_home)); then
  # Mount the disk array at /srv/archive and NVMe scratch at /srv/scratch
  # before or after running this; ownership is set either way.
  install -d -o 65532 -g 65532 -m 0755 /srv/archive /srv/scratch /srv/scratch/archive /srv/scratch/backfill
fi

# ---------------------------------------------------------------- wireguard
log "wireguard"
umask 077
if [[ ! -s /etc/wireguard/bleh0.key ]]; then
  wg genkey > /etc/wireguard/bleh0.key
fi
wg pubkey < /etc/wireguard/bleh0.key > /etc/wireguard/bleh0.pub
umask 022
echo "WireGuard public key: $(cat /etc/wireguard/bleh0.pub)"
echo "Configure peers with deploy/scripts/wg-mesh.sh (docs/operations/deploy.md)."

# ---------------------------------------------------------------- firewall
if ((firewall)); then
  log "nftables"
  ssh_set="0.0.0.0/0"
  ((${#ssh_cidrs[@]})) && ssh_set=$(IFS=,; echo "${ssh_cidrs[*]}")
  public_tcp="" public_udp="51820"
  ((has_node)) && public_tcp+="4001, 4002, "
  if [[ "$role" == relay ]]; then public_tcp+="80, 443, "; public_udp+=", 443"; fi
  if ((has_home)); then public_tcp+="6881, 4101, "; public_udp+=", 6881, 4101"; fi
  public_tcp=${public_tcp%, }
  {
    echo "#!/usr/sbin/nft -f"
    echo "# Managed by deploy/scripts/host-bootstrap.sh (role: $role). Only this"
    echo "# table is touched; Docker's own tables are left alone."
    echo "table inet bleh_filter"
    echo "delete table inet bleh_filter"
    echo "table inet bleh_filter {"
    echo "  chain input {"
    echo "    type filter hook input priority filter; policy drop;"
    echo "    iif lo accept"
    echo "    ct state established,related accept"
    echo "    ct state invalid drop"
    echo "    iifname \"bleh0\" accept"
    echo "    iifname \"docker0\" accept"
    echo "    iifname \"br-*\" accept"
    echo "    meta l4proto { icmp, ipv6-icmp } accept"
    echo "    ip saddr { $ssh_set } tcp dport 22 ct state new limit rate 30/minute accept"
    [[ -n "$public_tcp" ]] && echo "    tcp dport { $public_tcp } accept"
    echo "    udp dport { $public_udp } accept"
    echo "  }"
    echo "}"
  } > /etc/nftables.d-bleh.nft
  install -m 0644 /etc/nftables.d-bleh.nft /etc/bleh/firewall.nft && rm -f /etc/nftables.d-bleh.nft
  nft -c -f /etc/bleh/firewall.nft
  # Oracle Cloud's Ubuntu images ship iptables rules that reject everything
  # but SSH; they would override this policy. Replace them with an empty
  # INPUT chain (the bleh_filter table now enforces the policy).
  if [[ -f /etc/iptables/rules.v4 ]] && grep -q 'icmp-host-prohibited' /etc/iptables/rules.v4; then
    cp /etc/iptables/rules.v4 /etc/iptables/rules.v4.bleh-backup
    iptables -P INPUT ACCEPT && iptables -F INPUT
    printf '*filter\n:INPUT ACCEPT [0:0]\n:FORWARD ACCEPT [0:0]\n:OUTPUT ACCEPT [0:0]\nCOMMIT\n' > /etc/iptables/rules.v4
  fi
  nft -f /etc/bleh/firewall.nft
  grep -q 'include "/etc/bleh/firewall.nft"' /etc/nftables.conf || echo 'include "/etc/bleh/firewall.nft"' >> /etc/nftables.conf
  systemctl enable nftables
fi

# ---------------------------------------------------------------- systemd units
log "systemd timers"
units=(bleh-host-metrics.service bleh-host-metrics.timer)
((has_node)) && units+=(bleh-prune.service bleh-prune.timer bleh-schema-drift.service bleh-schema-drift.timer)
if ((has_home)) || [[ "$role" == relay ]]; then units+=(bleh-probe.service bleh-probe.timer); fi
((has_home)) && units+=(bleh-archive-daily.service bleh-archive-daily.timer bleh-archive-anchors.service bleh-archive-anchors.timer)
for u in "${units[@]}"; do install -m 0644 "$repo/deploy/systemd/$u" "/etc/systemd/system/$u"; done
[[ -f /etc/bleh/probe.env ]] || install -m 0644 "$repo/deploy/systemd/env/probe.example.conf" /etc/bleh/probe.env
if ((has_home)); then
  [[ -f /etc/bleh/archive.env ]] || install -m 0600 "$repo/deploy/systemd/env/archive.example.conf" /etc/bleh/archive.env
  install -d -m 0755 /var/lib/bleh/archive-pipeline
fi
systemctl daemon-reload
for u in "${units[@]}"; do [[ "$u" == *.timer ]] && systemctl enable --now "$u"; done

# ---------------------------------------------------------------- compose .env
log "deploy/compose/.env"
env_file="$repo/deploy/compose/.env"
[[ -f "$env_file" ]] || install -m 0600 "$repo/deploy/compose/env.example.conf" "$env_file"
set_env() {
  if grep -q "^$1=" "$env_file"; then sed -i "s|^$1=.*|$1=$2|" "$env_file"; else printf '%s=%s\n' "$1" "$2" >> "$env_file"; fi
}
[[ -n "$site" ]] && set_env BLEH_SITE "$site"
[[ -n "$overlay_ip" ]] && set_env BLEH_OVERLAY_IP "$overlay_ip"
set_env S3_ALLOW_HTTP false
if ((has_node)); then
  set_env HL_DATA_DIR /srv/hl/hl
  set_env HL_SNAPSHOT_DIR /srv/hl/snapshots
  # The origin's capture watermark lands here; it is hl-prune.sh's default
  # BLEH_RAW_DIR (and /etc/bleh/prune.env's).
  set_env BLEH_RAW_DIR /srv/bleh/raw
  set_env BLEH_READER_DIR /srv/bleh/reader
fi
if ((has_home)); then
  set_env ARCHIVE_MIRROR_DIR /srv/archive
  set_env ARCHIVE_SCRATCH_DIR /srv/scratch/archive
  set_env BACKFILL_SCRATCH_DIR /srv/scratch/backfill
  set_env PROMETHEUS_CONFIG ../observability/prometheus/prometheus.yml
fi
chmod 600 "$env_file"

cat <<EOF

Host prepared for role '$role'. Next:
  1. Peer the overlay:      deploy/scripts/wg-mesh.sh (docs/operations/deploy.md)
  2. Install secrets:       deploy/compose/secrets/ (docs/operations/keys.md)
  3. Fill .env:             $env_file (R2 endpoint and credentials)
  4. Start the role:        deploy/up.sh $role
EOF

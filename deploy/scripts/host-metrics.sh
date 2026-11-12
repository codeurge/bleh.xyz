#!/usr/bin/env bash
# Small host facts for node_exporter's textfile collector (every 15 min):
# pending reboot after security updates, and pending security updates.
set -uo pipefail
dir=${TEXTFILE_DIR:-/var/lib/node_exporter/textfile}
mkdir -p "$dir"
reboot=0
[[ -f /var/run/reboot-required ]] && reboot=1
pending=$( (apt-get -s -o Debug::NoLocking=true upgrade 2>/dev/null | grep -c '^Inst .*security') || true)
cat > "$dir/.bleh_host.prom.tmp" <<EOF
# HELP bleh_reboot_required 1 if the host needs a reboot to finish updates.
# TYPE bleh_reboot_required gauge
bleh_reboot_required $reboot
# HELP bleh_security_updates_pending Security updates not yet installed.
# TYPE bleh_security_updates_pending gauge
bleh_security_updates_pending ${pending:-0}
EOF
mv -f "$dir/.bleh_host.prom.tmp" "$dir/bleh_host.prom"

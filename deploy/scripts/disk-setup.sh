#!/usr/bin/env bash
# Build and mount the node data filesystem.
#
#   disk-setup.sh MOUNTPOINT auto|/dev/XXX
#
# auto: every whole NVMe disk with no partitions, no filesystem signature
# and no mount is used; two or more are striped (mdadm RAID 0: node output
# is re-creatable from peers and captured hourly, so speed beats
# redundancy). XFS, mounted noatime by UUID. Refuses to touch any disk that
# has data on it.
set -euo pipefail

mnt=${1:?mountpoint}
mode=${2:-auto}

candidates=()
if [[ "$mode" == auto ]]; then
  while read -r name type; do
    [[ "$type" == disk && "$name" == nvme* ]] || continue
    dev=/dev/$name
    # No partitions, no signature, not mounted, not part of an array.
    [[ $(lsblk -nro NAME "$dev" | wc -l) -eq 1 ]] || continue
    [[ -z "$(blkid -o value -s TYPE "$dev" 2>/dev/null)" ]] || continue
    [[ -z "$(lsblk -nro MOUNTPOINT "$dev")" ]] || continue
    candidates+=("$dev")
  done < <(lsblk -dnro NAME,TYPE)
else
  [[ -b "$mode" ]] || { echo "disk-setup: $mode is not a block device" >&2; exit 1; }
  [[ -z "$(blkid -o value -s TYPE "$mode" 2>/dev/null)" ]] || { echo "disk-setup: $mode already has a filesystem" >&2; exit 1; }
  candidates=("$mode")
fi

if ((${#candidates[@]} == 0)); then
  echo "disk-setup: no unused NVMe disk found" >&2
  exit 1
fi

if ((${#candidates[@]} == 1)); then
  target=${candidates[0]}
else
  target=/dev/md/bleh-hl
  mdadm --create "$target" --run --level=0 --raid-devices="${#candidates[@]}" "${candidates[@]}"
  mdadm --detail --scan | grep bleh-hl >> /etc/mdadm/mdadm.conf
  update-initramfs -u
fi

mkfs.xfs -f -L bleh-hl "$target"
uuid=$(blkid -o value -s UUID "$target")
mkdir -p "$mnt"
grep -q "$uuid" /etc/fstab || echo "UUID=$uuid $mnt xfs defaults,noatime,nofail 0 2" >> /etc/fstab
systemctl daemon-reload
mount "$mnt"
echo "disk-setup: $mnt on $target (${candidates[*]})"

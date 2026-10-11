#!/usr/bin/env bash
# End-to-end Secure Boot test of a stick made by `toastrack install`, in QEMU with Microsoft's keys.
# usage: e2e-secboot.sh <work-dir> <ubuntu.iso> [stick-size]      (then drive the menu in the QEMU window/monitor)
# DATA_FS=fat32 uses a FAT32 data partition (ISO < 4 GB, copied with mtools); default exfat needs root,
# losetup and exfat-fuse to copy the ISO in.
# needs: qemu-system-x86 ovmf mtools xorriso sbsigntool openssl, `pip install virt-firmware`, built loader + CLI
set -euo pipefail
work=$1; iso=$2; size=${3:-6G}
cd "$(dirname "$0")/.."
export MTOOLS_SKIP_CHECK=1
scripts/make-dev-bundle.sh "$work/bundle" "$iso" >/dev/null
rm -f "$work/stick.img"
fs=${DATA_FS:-exfat}
cargo run -q -p toastrack -- install "$work/stick.img" --size "$size" --bundle "$work/bundle" --data-fs "$fs"
# Copy the ISO into the data partition (second GPT partition, 65 MiB offset with the default layout).
off=$((65 * 1024 * 1024))
if [ "$fs" = fat32 ]; then
  mcopy -i "$work/stick.img@@$off" "$iso" ::/isos/
else
  lo=$(losetup -f --show -o "$off" "$work/stick.img")
  mkdir -p "$work/mnt"
  mount.exfat-fuse "$lo" "$work/mnt"
  cp "$iso" "$work/mnt/isos/"
  sync
  umount "$work/mnt"
  losetup -d "$lo"
fi
cargo run -q -p toastrack -- verify "$work/stick.img" --bundle "$work/bundle"
# The VM's firmware variables get the dev certificate as a MOK, standing in for the MokManager step.
virt-fw-vars -i /usr/share/OVMF/OVMF_VARS_4M.ms.fd -o "$work/vars.fd" \
  --add-mok 5b2f9ae1-1c0d-4d0e-9c3a-0a1b2c3d4e5f "$work/bundle/cert.pem" >/dev/null 2>&1
SECBOOT=1 VARS="$work/vars.fd" exec scripts/qemu.sh "$work/stick.img" "${@:4}"

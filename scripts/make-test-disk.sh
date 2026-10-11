#!/usr/bin/env bash
# Build a FAT32 test disk (label TOASTRACK) with the loader as the default EFI app.
# usage: make-test-disk.sh <out.img> <size-MB> [iso...]   (ISOs are copied to /isos/linux/)
set -euo pipefail
out=$1; size=$2; shift 2
loader=${LOADER:-target/x86_64-unknown-uefi/debug/toastrack-loader.efi}
rm -f "$out"; truncate -s "${size}M" "$out"
mkfs.fat -F32 -n TOASTRACK "$out" >/dev/null
export MTOOLS_SKIP_CHECK=1
mmd -i "$out" ::/EFI ::/EFI/BOOT ::/isos ::/isos/linux ::/isos/windows
mcopy -i "$out" "$loader" ::/EFI/BOOT/BOOTX64.EFI
for iso in "$@"; do mcopy -i "$out" "$iso" ::/isos/linux/; done

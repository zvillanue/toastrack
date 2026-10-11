#!/usr/bin/env bash
# Run a disk image under QEMU + OVMF. usage: qemu.sh <disk.img> [extra qemu args]
# SECBOOT=1 uses the Secure Boot firmware (with Microsoft keys enrolled).
set -euo pipefail
img=$1; shift
d=/usr/share/OVMF
code=$d/OVMF_CODE_4M.fd; vars=$d/OVMF_VARS_4M.fd
if [ "${SECBOOT:-0}" = 1 ]; then code=$d/OVMF_CODE_4M.secboot.fd; vars=$d/OVMF_VARS_4M.ms.fd; fi
[ -n "${VARS:-}" ] && vars=$VARS
cp "$vars" "${VARS_COPY:-/tmp/ovmf_vars.fd}"
exec qemu-system-x86_64 -machine q35 -m 3072 -accel tcg -smp 2 \
  -drive if=pflash,format=raw,readonly=on,file="$code" \
  -drive if=pflash,format=raw,file="${VARS_COPY:-/tmp/ovmf_vars.fd}" \
  -drive file="$img",format=raw,if=none,id=disk0,snapshot=on -device ide-hd,drive=disk0 \
  -device qemu-xhci -device usb-kbd -device usb-mouse -device VGA \
  "$@"

#!/usr/bin/env bash
# Prepare a Secure Boot test: shim (Microsoft-signed, taken from an Ubuntu ISO) -> Toastrack loader
# signed with a throw-away key. The key is enrolled in the VM's db, standing in for a one-time MOK enrollment.
# usage: secboot-setup.sh <disk.img> <ubuntu.iso> <workdir>
# needs: xorriso sbsigntool openssl mtools virt-firmware (pip)
set -euo pipefail
img=$1; iso=$2; work=$3; mkdir -p "$work"
loader=${LOADER:-target/x86_64-unknown-uefi/debug/toastrack-loader.efi}
export MTOOLS_SKIP_CHECK=1
[ -f "$work/key.pem" ] || openssl req -new -x509 -newkey rsa:2048 -nodes -sha256 -days 365 \
  -subj "/CN=Toastrack Test Signing Key/" -keyout "$work/key.pem" -out "$work/cert.pem" 2>/dev/null
xorriso -osirrox on -indev "$iso" -extract /EFI/boot/bootx64.efi "$work/shimx64.efi" \
  -extract /EFI/boot/mmx64.efi "$work/mmx64.efi" 2>&1 | grep -i "fail" || true
sbsign --key "$work/key.pem" --cert "$work/cert.pem" --output "$work/loader-signed.efi" "$loader" >/dev/null
mcopy -o -i "$img" "$work/shimx64.efi" ::/EFI/BOOT/BOOTX64.EFI
mcopy -o -i "$img" "$work/mmx64.efi" ::/EFI/BOOT/mmx64.efi
# UNSIGNED=1 installs the unsigned loader instead (negative test: shim must refuse it).
if [ "${UNSIGNED:-0}" = 1 ]; then src=$loader; else src=$work/loader-signed.efi; fi
mcopy -o -i "$img" "$src" ::/EFI/BOOT/grubx64.efi
# Firmware variables: Microsoft keys, plus our certificate enrolled either as a MOK (default, what real
# users get after MokManager) or in the firmware db (ENROLL=db), or not at all (ENROLL=none).
args=(-i /usr/share/OVMF/OVMF_VARS_4M.ms.fd -o "$work/vars.fd")
case "${ENROLL:-mok}" in
  mok) args+=(--add-mok 5b2f9ae1-1c0d-4d0e-9c3a-0a1b2c3d4e5f "$work/cert.pem") ;;
  db)  args+=(--add-db 5b2f9ae1-1c0d-4d0e-9c3a-0a1b2c3d4e5f "$work/cert.pem") ;;
esac
# SHIM_VERBOSE=1 makes shim log its decisions to the serial console.
if [ "${SHIM_VERBOSE:-0}" = 1 ]; then
  echo '{"version":2,"variables":[{"name":"SHIM_VERBOSE","guid":"605dab50-e046-4300-abb6-3dd810dd8b23","attr":3,"data":"01"}]}' > "$work/verbose.json"
  args+=(--set-json "$work/verbose.json")
fi
virt-fw-vars "${args[@]}" >/dev/null 2>&1
echo "ready: $work/vars.fd"

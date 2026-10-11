#!/usr/bin/env bash
# Build a DEVELOPMENT bundle for `toastrack install`: Microsoft-signed shim taken from an Ubuntu ISO,
# the loader signed with a throw-away key (kept in <out>/ so it can be enrolled and reused).
# Real releases sign with the project's release key instead.
# usage: make-dev-bundle.sh <out-dir> <ubuntu.iso> [loader.efi]
# If the Windows helper builds (rustup target x86_64-pc-windows-gnu + mingw), it is added as toastrack-attach.exe.
set -euo pipefail
out=$1; iso=$2
loader=${3:-target/x86_64-unknown-uefi/debug/toastrack-loader.efi}
mkdir -p "$out"
[ -f "$out/key.pem" ] || openssl req -new -x509 -newkey rsa:2048 -nodes -sha256 -days 365 \
  -subj "/CN=Toastrack Dev Signing Key/" -keyout "$out/key.pem" -out "$out/cert.pem" 2>/dev/null
xorriso -osirrox on -indev "$iso" -extract /EFI/boot/bootx64.efi "$out/shimx64.efi" \
  -extract /EFI/boot/mmx64.efi "$out/mmx64.efi" >/dev/null 2>&1
chmod u+w "$out"/*.efi
sbsign --key "$out/key.pem" --cert "$out/cert.pem" --output "$out/grubx64.efi" "$loader" >/dev/null 2>&1
openssl x509 -in "$out/cert.pem" -outform DER -out "$out/ENROLL_THIS_KEY_IN_MOKMANAGER.cer"
if cargo build --release -q -p toastrack-winattach --target x86_64-pc-windows-gnu 2>/dev/null; then
  cp target/x86_64-pc-windows-gnu/release/toastrack-attach.exe "$out/toastrack-attach.exe"
else
  echo "note: Windows helper not built (needs mingw + the x86_64-pc-windows-gnu target); Windows ISOs will not install" >&2
fi
ls -1 "$out"

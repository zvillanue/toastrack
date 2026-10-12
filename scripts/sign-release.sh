#!/usr/bin/env bash
# Turn the unsigned build artifacts of a release into signed, ready-to-publish packages. Run on the machine that holds the key.
# usage: sign-release.sh <version> <artifacts-dir> <key.pem> <cert.pem> [out-dir (default: dist)]
#
# <artifacts-dir> is what the release workflow produced (download it with `gh run download <id> -D artifacts`):
#   parts/toastrack-loader.efi  parts/toastrack-attach.exe
#   app-linux-x86_64/{toastrack,toastrack-gui}   app-windows-x86_64/{toastrack.exe,toastrack-gui.exe}
# needs: sbsign (sbsigntool), openssl, zip, tar, curl, dpkg-deb, sha256sum
set -euo pipefail
ver=${1:?version, e.g. 0.2.0}; art=$(cd "${2:?artifacts dir}" && pwd); key=${3:?key.pem}; cert=${4:?cert.pem}
out=${5:-dist}
here=$(cd "$(dirname "$0")/.." && pwd)
for t in sbsign openssl zip tar curl dpkg-deb sha256sum; do command -v "$t" >/dev/null || { echo "missing tool: $t" >&2; exit 1; }; done
mkdir -p "$out"; out=$(cd "$out" && pwd)
work=$(mktemp -d); trap 'rm -rf "$work"' EXIT
b=$work/bundle

echo "== shim (pinned, unmodified)"
"$here/scripts/fetch-shim.sh" "$b"

echo "== signing the loader"
sbsign --key "$key" --cert "$cert" --output "$b/grubx64.efi" "$art/parts/toastrack-loader.efi"
openssl x509 -in "$cert" -outform DER -out "$b/ENROLL_THIS_KEY_IN_MOKMANAGER.cer"
cp "$art/parts/toastrack-attach.exe" "$b/toastrack-attach.exe"
fp=$(openssl x509 -in "$cert" -noout -fingerprint -sha256 | cut -d= -f2)
printf 'Toastrack %s\nSigning key SHA-256 fingerprint: %s\n' "$ver" "$fp" > "$b/VERSION.txt"

echo "== checking the bundle (the installer's own validation, then a trial install and read-back)"
cli=$art/app-linux-x86_64/toastrack; chmod +x "$cli"
"$cli" install "$work/trial.img" --size 400M --bundle "$b" --yes >/dev/null
"$cli" verify "$work/trial.img" --bundle "$b"
rm -f "$work/trial.img"

echo "== packaging"
doc() { cp "$here/LICENSE" "$here/release/README-RELEASE.txt" "$1/"; cp -r "$b/licenses" "$1/licenses"; }
pkg=$work/pkg; rm -rf "$pkg"
# Windows
d=$pkg/toastrack-$ver-windows-x86_64; mkdir -p "$d"
cp "$art"/app-windows-x86_64/toastrack.exe "$art"/app-windows-x86_64/toastrack-gui.exe "$d/"
cp -r "$b" "$d/bundle"; doc "$d"
(cd "$pkg" && zip -qr "$out/toastrack-$ver-windows-x86_64.zip" "toastrack-$ver-windows-x86_64")
# Linux
d=$pkg/toastrack-$ver-linux-x86_64; mkdir -p "$d"
cp "$art"/app-linux-x86_64/toastrack "$art"/app-linux-x86_64/toastrack-gui "$d/"; chmod +x "$d/toastrack" "$d/toastrack-gui"
cp -r "$b" "$d/bundle"; doc "$d"
(cd "$pkg" && tar -czf "$out/toastrack-$ver-linux-x86_64.tar.gz" "toastrack-$ver-linux-x86_64")
# just the bundle (macOS, other tools, `toastrack install --bundle`)
d=$pkg/toastrack-$ver-bundle; cp -r "$b" "$d"; doc "$d"
(cd "$pkg" && zip -qr "$out/toastrack-$ver-bundle.zip" "toastrack-$ver-bundle")

echo "== checksums and signature"
(cd "$out" && sha256sum "toastrack-$ver"-* > SHA256SUMS && openssl dgst -sha256 -sign "$key" -out SHA256SUMS.sig SHA256SUMS)
openssl x509 -in "$cert" -pubkey -noout > "$out/toastrack-release-key.pub"
echo
echo "Done. Upload everything in $out to the GitHub release for v$ver:"
ls -1 "$out"
echo "Verify a download: openssl dgst -sha256 -verify toastrack-release-key.pub -signature SHA256SUMS.sig SHA256SUMS"
echo "Key fingerprint to state in the release notes: $fp"

#!/usr/bin/env bash
# Download the pinned Ubuntu shim-signed package (release/shim.lock), check its checksum and put the unmodified,
# Microsoft-signed shim and MokManager plus the license notices into <outdir>.
# usage: fetch-shim.sh <outdir>
# needs: curl, dpkg-deb (apt install dpkg on non-Debian systems), sha256sum
set -euo pipefail
out=${1:?usage: fetch-shim.sh <outdir>}
here=$(cd "$(dirname "$0")/.." && pwd)
# shellcheck disable=SC1091
. "$here/release/shim.lock"
mkdir -p "$out/licenses"
tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
curl -fsSL "$SHIM_DEB_URL" -o "$tmp/shim.deb"
echo "$SHIM_DEB_SHA256  $tmp/shim.deb" | sha256sum -c - >/dev/null || { echo "shim package checksum mismatch: refusing it" >&2; exit 1; }
dpkg-deb -x "$tmp/shim.deb" "$tmp/x"
# shimx64.efi.signed.latest is the Microsoft-signed build; mmx64.efi is MokManager (signed with Canonical's key, which this shim trusts)
cp "$tmp/x/usr/lib/shim/shimx64.efi.signed.latest" "$out/shimx64.efi"
cp "$tmp/x/usr/lib/shim/mmx64.efi" "$out/mmx64.efi"
cp "$tmp/x/usr/share/doc/shim-signed/copyright" "$out/licenses/shim-copyright.txt"
cat > "$out/licenses/shim-NOTICE.txt" <<NOTICE
shimx64.efi and mmx64.efi in this bundle are the unmodified binaries from Ubuntu's shim-signed package $SHIM_VERSION
(shimx64.efi is shimx64.efi.signed.latest). shim is licensed under the BSD-2-Clause license, see shim-copyright.txt.
Source code: https://launchpad.net/ubuntu/+source/shim and https://launchpad.net/ubuntu/+source/shim-signed (upstream:
https://github.com/rhboot/shim). Package SHA-256: $SHIM_DEB_SHA256
NOTICE
echo "shim $SHIM_VERSION -> $out"

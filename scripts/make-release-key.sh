#!/usr/bin/env bash
# Create the Toastrack release signing key (run once, on an offline machine you trust).
# usage: make-release-key.sh <dir>     -> <dir>/key.pem (passphrase-protected), cert.pem, ENROLL_THIS_KEY_IN_MOKMANAGER.cer
#
# Whoever holds key.pem can sign boot code that every PC with this key enrolled will run. Keep it offline (an encrypted
# USB stick, a hardware token), back it up, and never put it in a repository, a CI secret or a chat. See docs/RELEASING.md.
set -euo pipefail
dir=${1:?usage: make-release-key.sh <dir>}
[ ! -e "$dir/key.pem" ] || { echo "$dir/key.pem exists; refusing to overwrite a signing key" >&2; exit 1; }
mkdir -p "$dir"; chmod 700 "$dir"
openssl req -new -x509 -newkey rsa:4096 -sha256 -days 3650 \
  -subj "/CN=Toastrack Release Signing Key/O=Toastrack" \
  -addext "basicConstraints=critical,CA:FALSE" -addext "keyUsage=critical,digitalSignature" -addext "extendedKeyUsage=codeSigning" \
  -keyout "$dir/key.pem" -out "$dir/cert.pem"          # asks for a passphrase: choose a long one
chmod 600 "$dir/key.pem"
openssl x509 -in "$dir/cert.pem" -outform DER -out "$dir/ENROLL_THIS_KEY_IN_MOKMANAGER.cer"
echo
echo "Key created in $dir"
echo "SHA-256 fingerprint (publish it in the README; MokManager shows the same value when users enroll the key):"
openssl x509 -in "$dir/cert.pem" -noout -fingerprint -sha256
echo "Valid until: $(openssl x509 -in "$dir/cert.pem" -noout -enddate | cut -d= -f2)"

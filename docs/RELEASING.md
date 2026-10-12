# Releasing

How a Toastrack release is built, signed and published, and how the signing key is handled. Maintainers only.

## What is being protected
Every PC that enrolls the release certificate (`ENROLL_THIS_KEY_IN_MOKMANAGER.cer`) will run any boot code signed with the
matching private key, with Secure Boot **on**. **Whoever holds the private key can run code before the operating system on all
those PCs.** Treat it like a root CA key:
- It is created offline (`scripts/make-release-key.sh`), passphrase-protected, and kept on an encrypted offline medium with a
  second backup in another place. It never goes into the repository, a CI secret, a chat or an agent session.
- Signing happens only on the maintainer's machine, on binaries built by CI from a tagged commit.
- The key's SHA-256 fingerprint is published in the README and in every release's notes, and printed in the app and in
  `bundle/VERSION.txt`, so users can compare it with what MokManager shows before they enroll.

## One-time setup
```sh
scripts/make-release-key.sh /path/to/offline/toastrack-key        # asks for a passphrase; prints the fingerprint
```
Put the printed fingerprint into README.md ("Release key") in a commit. Back the folder up.

## Cutting a release
1. Update the version in the `Cargo.toml` files and `CHANGELOG.md`; make sure CI is green; merge to `main`.
2. `git tag -a vX.Y.Z -m "Toastrack X.Y.Z" && git push origin vX.Y.Z`. The *Release build* workflow builds the unsigned
   loader, the Windows helper and the Linux and Windows programs, and attaches `toastrack-vX.Y.Z-unsigned-build.zip` to a **draft**
   release.
3. On the signing machine: download and unpack that zip into an empty folder, then
   ```sh
   scripts/sign-release.sh X.Y.Z <folder> /path/to/key.pem /path/to/cert.pem dist
   ```
   It downloads the pinned shim (`release/shim.lock`, checksum-verified), signs the loader, runs the installer's own validation plus a
   trial install and read-back, builds `toastrack-X.Y.Z-windows-x86_64.zip`, `-linux-x86_64.tar.gz`, `-bundle.zip`, `SHA256SUMS`,
   `SHA256SUMS.sig` (signed with the release key) and `toastrack-release-key.pub`.
4. **Test the signed result before publishing**: write a stick from the package (`toastrack-gui` or `toastrack install`),
   enroll the key on a PC or VM with Secure Boot on, boot an ISO. `scripts/e2e-secboot.sh` automates this in QEMU for a bundle.
5. Replace the draft release's assets with the files from `dist/`, put the key fingerprint in the notes, publish.

Verify a download: `openssl dgst -sha256 -verify toastrack-release-key.pub -signature SHA256SUMS.sig SHA256SUMS`, then
`sha256sum -c SHA256SUMS`. (The public key file is itself only trustworthy if obtained from the repository at the tag; the fingerprint
in the README is the anchor.)

## Shim
Releases redistribute the Microsoft-signed shim **unmodified** from Ubuntu's `shim-signed` package, pinned by URL and checksum in
`release/shim.lock`. shim is BSD-2-Clause; `fetch-shim.sh` includes the copyright file and a source notice in `bundle/licenses`.
Update the pin when Ubuntu ships a new `shim-signed` (security fixes, or SBAT/dbx revocations make old shims unbootable): change the three
values, re-run a Secure Boot boot test, release. A shim that Microsoft later revokes stops booting on PCs that apply the revocation, so
watch https://launchpad.net/ubuntu/+source/shim-signed and Microsoft's dbx updates.
Not planned for now: a Toastrack-owned shim submitted to shim-review (would remove the enrollment step; needs a legal entity and a
reproducible shim build).

## If the key is lost or leaked
- **Lost** (no leak): generate a new key, release as normal; every PC must enroll the new certificate once. Old sticks keep working
  on PCs that still have the old key.
- **Leaked or suspected**: stop releasing, announce it publicly, publish a new key and release, and tell users to delete the old key from
  their PCs (MokManager: *Delete key*, or `mokutil --delete ENROLL_THIS_KEY_IN_MOKMANAGER.cer`). Bump the loader's SBAT generation
  (`SBAT_TEXT` in `crates/loader/src/main.rs`) in the new release so revocation tooling has something to key on.

## Known gaps
- Linux/Windows x86-64 programs only; no macOS package yet (the `-bundle.zip` plus `toastrack install` on a Linux machine works).
- The Windows programs are not Authenticode-signed (SmartScreen will warn). Needs a code-signing certificate.
- Builds are not bit-for-bit reproducible yet (toolchain `stable`, not pinned).

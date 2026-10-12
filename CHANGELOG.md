# Changelog

## Unreleased
- Graphical stick maker `toastrack-gui` (Windows and Linux): pick the stick, options, Write; update-only mode; image-file mode
  for VMs; shows the signing key fingerprint. Windows build asks for administrator rights (manifest); Linux can restart through polkit.
- Windows physical-drive support in the installer and CLI (`toastrack list`, `install`, `update`, `verify` on `\\.\PhysicalDriveN`):
  volumes are locked and dismounted, the system disk is never offered, writes are sector-aligned. **Not yet tried on a real Windows PC.**
- `toastrack` finds the `bundle` folder next to the program by default.
- Release pipeline: GitHub Actions CI and release-build workflows, pinned Ubuntu shim (`release/shim.lock`, `scripts/fetch-shim.sh`),
  offline signing scripts (`make-release-key.sh`, `sign-release.sh`) producing Windows/Linux packages with a signed `SHA256SUMS`;
  documented in docs/RELEASING.md. Dry-run locally: package stick boots under Secure Boot in QEMU. Workflows not yet run on GitHub.

## 0.1.0 - first release
Menu and boot
- Native UEFI menu (Rust, GOP rendering), folder browsing, keyboard control; per-ISO Secure Boot badge (`SECURE BOOT OK`,
  `NEEDS KEY ENROLL`, `UNSIGNED`, `NO UEFI LOADER`), `WIN` tag for Windows ISOs.
- Secure Boot chain: Microsoft-signed shim -> Toastrack loader (your key, enrolled once per PC via MokManager) -> the ISO's own
  signed GRUB with a generated single-entry `grub.cfg` served from a virtual disk; direct kernel start when Secure Boot is off.
- Virtual optical disk (BlockIo + device path) so the firmware's ISO 9660/UDF/El Torito drivers see the ISO.
- Distro profiles: Ubuntu family (casper), Debian live, Fedora/RHEL live (dracut), Fedora-style installers, Arch family.
- exFAT data partition by default (files over 4 GB) with a read-only exFAT driver; FAT32 optional. For exFAT the ISO's raw
  location is passed to casper / live-boot, which requires the ISO to be one contiguous run.
- ISO integrity check: press `C` to verify SHA-256 against `<name>.sha256` / `SHA256SUMS` next to the ISO; `SUM OK` / `DAMAGED` tags.
- El Torito and FAT readers (loaders inside the boot image), read-only UDF reader, WIM/LZX reader.

Windows (experimental)
- Windows 10/11 ISOs: loader publishes a hand-off variable; `autounattend.xml` + `toastrack-attach.exe` on the stick attach the ISO
  as a virtual DVD so Setup finds `install.wim`. Reached Setup's disk selection in VirtualBox with Secure Boot off.
- `toastrack windows-report` to describe a Windows ISO.

Tools
- `toastrack` CLI: `list`, `install`, `update`, `verify`, `check`, `explain`, `check-volume`, `windows-report`.
- Installer with its own GPT writer, FAT32 and exFAT formatters, bundle validation, device safety checks.
- Scripts for a development bundle and QEMU/OVMF Secure Boot testing.

Known limitations
- No mouse (attempted, removed: no pointer events from the test machines' firmware).
- Installer is Linux only; no pre-built release bundle (build your own with `scripts/make-dev-bundle.sh`).
- Full Linux boots verified in QEMU/OVMF; real-PC boots and Secure Boot with Windows ISOs not yet verified.
- See the README and docs/ROADMAP.md for the full list.

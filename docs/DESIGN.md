# Toastrack design

A Ventoy-style multiboot USB tool for UEFI machines, with Secure Boot support,
a Secure Boot status badge per image, folder browsing and mouse support.

## Decisions
- UEFI x86-64 only (no BIOS).
- Secure Boot: Microsoft-signed **shim** (taken from a distro) -> Toastrack loader signed with
  our own key. First boot on each machine needs a one-time MOK enrollment (as with Ventoy).
  Phase 2 (optional): submit our own shim to shim-review for Microsoft signing.
- Menu: native UEFI app in Rust (`uefi-rs`), GOP graphics + Simple Pointer for mouse.
- Installer runs on Windows and Linux. Main payload: mainstream Linux ISOs.

## Stick layout
1. Partition 1 (exFAT, rest of the disk): user ISOs, any folder structure.
2. Partition 2 (FAT32, ~64 MB, ESP): `EFI/BOOT/BOOTX64.EFI` (shim), `grubx64`-slot = Toastrack loader, MOK enroll cert.

## Booting an ISO under Secure Boot
Loader exposes the ISO as a virtual block device (`EFI_BLOCK_IO` via a small
memory-mapped/file-backed driver), then chainloads the ISO's own
`EFI/BOOT/BOOTX64.EFI`. Ubuntu/Fedora/Debian ship a Microsoft-signed shim there, so the
firmware's normal trust chain applies. Unsigned images cannot be booted with Secure Boot on;
the menu shows that instead of failing at boot time.

## Secure Boot badge (`toastrack-core`)
Per ISO: read `EFI/BOOT/BOOTX64.EFI`, parse PE Authenticode, classify:
`Trusted` (Microsoft UEFI CA in chain) / `NeedsEnrollment` / `Unsigned` / `NoUefiLoader`.

## Status
- [x] `toastrack-core`: ISO 9660 walker, PE signer extraction, verdict, synthetic tests.
- [x] `toastrack check <iso>...` CLI.
- [x] `toastrack-loader`: builds for `x86_64-unknown-uefi`; skeleton only.
- [ ] Not yet tested against real signed ISOs (no ISO tooling in the dev sandbox).
- [ ] Not yet done: El Torito FAT image fallback, DBX revocation check, virtual disk driver,
      menu UI / mouse, exFAT+ESP installer, shim/MOK packaging, Windows GUI.

## Building
    cargo test
    cargo run -p toastrack -- check some.iso
    rustup target add x86_64-unknown-uefi
    cargo build -p toastrack-loader --target x86_64-unknown-uefi --release

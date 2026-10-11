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
Verified in QEMU/OVMF with the real Ubuntu 24.04.4 server ISO:
- [x] Menu: folder tree, ISO list, per-ISO Secure Boot badge computed on-device, keyboard + mouse (relative and absolute pointers).
- [x] Virtual optical disk (`vdisk.rs`): firmware drivers build filesystems on it and the ISO's own
      signed shim -> GRUB -> kernel start.
- [x] **Ubuntu boots fully from its ISO file.** The loader reads the ISO's `boot/grub/loopback.cfg`
      (generic, not Ubuntu-specific), loads kernel + initrd from the ISO itself, hands the initrd to the
      Linux EFI stub via the `LINUX_EFI_INITRD_MEDIA_GUID` LoadFile2 protocol, and starts the kernel with
      `iso-scan/filename=<path of the ISO on the stick>`. Verified: casper finds the ISO, systemd reaches
      `getty.target` (Ubuntu Server 24.04.4, QEMU/OVMF, Secure Boot off).
- [ ] Never run under Secure Boot yet (needs loader signing + key enrollment test setup).
- [ ] exFAT: firmware has no exFAT driver, so today the data partition must be FAT32 (4 GB file limit).
- [ ] Not yet done: El Torito fallback for ISOs whose loader is only inside the boot image, DBX
      revocation check, installer, shim/MOK packaging, Windows ISO support.

## Stage 2: making the OS find its ISO
Chainloading only works until ExitBootServices (that is why the direct-kernel path exists). Status: step 2 below is built and working for Ubuntu; the rest are not.
1. Per-distro profiles: load kernel + initrd ourselves with the right cmdline
   (Ubuntu `iso-scan/filename=`, Debian live `findiso=`, Arch `img_dev=`/`img_loop=`, Fedora `rd.live.image` +
   `iso-scan/filename=`), under shim's verification hook so Secure Boot still applies.
   Needs the data partition to be readable by Linux (FAT32/exFAT) - it is.
2. Use an ISO's own `boot/grub/loopback.cfg` when present (the standard for this).
3. Windows: copy-to-RAM approach (wimboot-style) - separate project.

## Building
    cargo test
    cargo run -p toastrack -- check some.iso
    rustup target add x86_64-unknown-uefi
    cargo build -p toastrack-loader --target x86_64-unknown-uefi --release

    # try it in QEMU (needs qemu-system-x86, ovmf, mtools, dosfstools)
    scripts/make-test-disk.sh /tmp/test.img 4096 some.iso
    scripts/qemu.sh /tmp/test.img

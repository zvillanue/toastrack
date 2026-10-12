# Roadmap

Ideas for 0.2.0 and beyond, roughly in the order they are worth doing. Nothing here is promised.

## 0.2.0 candidates
1. **Prove it on real hardware.** Boot a complete Linux ISO under Secure Boot on at least two physical PCs, including the
   MokManager enrollment on real firmware (only QEMU and VirtualBox have exercised it). Fix what that finds.
2. **Real-ISO coverage.** Boot official Debian live, Fedora Workstation/Server, Linux Mint, Kali, Arch/EndeavourOS and an
   Alma/Rocky installer ISO; today only Ubuntu is verified with a real ISO, the others use stand-ins or documentation.
3. **Windows installer for the stick** (device access on Windows, a small GUI) so Windows users can prepare a stick without Linux.
   A GUI for Linux/macOS later.
4. **Release signing and distribution.** A project signing key, signed release bundles (so users do not need `sbsigntools`, a
   throw-away key or an Ubuntu ISO), reproducible builds, and a decision on shipping a shim or submitting to shim-review.
5. **Windows:** finish verification (Secure Boot on, real hardware, a completed install), Windows 10, `install.esd` ISOs,
   and a clearer way to avoid picking the stick's own partitions in Setup.
6. **Fragmented ISOs on exFAT.** Today an ISO must be contiguous for initramfs-side lookup. Options: have the installer/updater
   defragment (an exFAT *writer* in the loader or a helper command `toastrack add iso`), or pass a loop-device map.
7. **A pre-boot ISO check on demand and in bulk:** verify all ISOs in a folder, remember results on the stick, optionally verify
   against published signatures (GPG) instead of only hashes.
8. **Mouse support, revisited.** Needs real-hardware investigation first: a firmware that exposes working pointer devices, or a
   direct HID path on an xHCI/EHCI implementation that does it. The removed attempt is in history (`7e76dc1`).
9. **Better menu:** search/filter, sort, long names, an on-screen help page, remembering the last choice, a default timeout.

## Later
- Persistence for Ubuntu/Debian live (a persistence file on the data partition and the kernel arguments for it).
- Per-ISO custom kernel parameters (a small `toastrack.cfg`) and ISO descriptions.
- Boot raw disk images (IMG/VHD) and EFI tools (memtest, rescue) next to ISOs.
- dbx awareness: warn when a Windows or shim loader is revoked on this PC.
- `toastrack update` for the data partition (exFAT writer), update checks.
- ARM64 (aarch64 UEFI) build of the loader.
- XPRESS and LZMS decompression in the WIM reader (for `install.esd`).
- Localised UI and bigger text for high-DPI screens.

## Known issues being tracked
- No mouse (see above).
- Booting under Secure Boot depends on the ISO's own signed GRUB and a rewritable `grub.cfg`; ISOs that load their kernel
  differently (some Arch/rescue ISOs) may need another approach.
- The Secure Boot badge cannot predict PC-specific revocations.

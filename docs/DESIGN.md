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
- [x] **Secure Boot ON, end to end (QEMU/OVMF with Microsoft keys, `scripts/secboot-setup.sh`):**
      firmware -> Ubuntu's Microsoft-signed shim 15.8 -> Toastrack loader (our test key, enrolled as a MOK)
      -> menu -> Ubuntu boots from the ISO file to a live system. Also verified: shim refuses an unsigned
      loader, and refuses a signed loader whose key is not enrolled.
- [x] Loader carries an `.sbat` section; current shim refuses second-stage loaders without one.
- [ ] Not yet tested: the interactive first-boot enrollment through MokManager (the test writes the MOK
      list into the VM's variables directly), real hardware, other firmware.

### How kernels are started under Secure Boot (and what did not work)
The loader cannot start a distro kernel itself under Secure Boot:
- Firmware `LoadImage` rejects it (the distro key is in shim, not in the firmware db).
- Asking shim to vouch for it fails: Ubuntu's shim 15.8 answers every `SHIM_LOCK.Verify()` call from our
  loader with `EFI_UNSUPPORTED` (even for Microsoft- and Canonical-signed files, called before or after any
  `LoadImage`, MOK or db enrollment, with shim's verbose log showing nothing). Cause not found; GRUB's calls
  to the same function work. Code for the shim/security-hook route was removed.

What works instead: with Secure Boot on, the loader chainloads the ISO's own signed shim -> GRUB from the
virtual disk and serves **a rewritten `boot/grub/grub.cfg`** (the virtual disk overlays those bytes, the ISO
file is untouched) containing only the entry chosen in Toastrack's menu, with `iso-scan/filename=` filled in.
The distro's signed GRUB then does the kernel verification itself, exactly as on a normal install medium.
With Secure Boot off the loader starts the kernel directly (faster, no dependence on the ISO's GRUB).
Limits: the generated config must fit in the ISO's existing `grub.cfg` size (true for Ubuntu), and the ISO
must ship `boot/grub/grub.cfg` and a signed GRUB.

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

## Secure Boot test setup
    scripts/make-test-disk.sh /tmp/test.img 4096 ubuntu.iso
    scripts/secboot-setup.sh /tmp/test.img ubuntu.iso /tmp/sb        # shim from the ISO, signs the loader, builds vars.fd
    SECBOOT=1 VARS=/tmp/sb/vars.fd scripts/qemu.sh /tmp/test.img     # ENROLL=db|none, UNSIGNED=1 for negative tests
Needs `sbsigntool openssl xorriso` and `pip install virt-firmware`. Ubuntu's shim is redistributed unmodified
(as distributions do); its Microsoft signature is what makes the chain work on stock firmware.

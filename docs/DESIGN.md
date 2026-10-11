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

## Stick layout (as written by `toastrack install`)
GPT, 1 MiB aligned, 512-byte sectors:
1. `TOASTRACK-ESP` - 64 MiB, FAT32, label `TSTRK-ESP`: `EFI/BOOT/BOOTX64.EFI` (Microsoft-signed shim),
   `mmx64.efi` (MokManager), `grubx64.efi` (the signed Toastrack loader: shim's default second stage),
   `ENROLL_THIS_KEY_IN_MOKMANAGER.cer` (release certificate, DER).
2. `TOASTRACK` - rest of the disk, FAT32, label `TOASTRACK` (the loader finds its data by this label):
   `isos/` plus anything the user copies. FAT32 for now (4 GB per file; exFAT needs a reader in the loader).
Volume serial numbers are random per stick.

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

- [x] **exFAT data partition (default), files over 4 GB.** The firmware has no exFAT driver, so the loader reads
      the partition itself: `toastrack-core::exfat` (read-only, `no_std`, FAT chains with run coalescing) over
      raw BlockIO. Verified against images made by exfatprogs `mkfs.exfat` and written by `exfat-fuse` (nested
      folders, unicode names, 300-entry directories, deliberately fragmented files, 4 KiB and 32 KiB
      clusters), and `fsck.exfat` accepts what the installer's own formatter (`exfat_format.rs`) writes. Booted
      Ubuntu from a 5.5 GB ISO on exFAT with Secure Boot ON and OFF.
      Linux's initramfs cannot mount exFAT (no module in Ubuntu's initrd), so for exFAT the loader passes the
      ISO's raw location instead of a path: `live-media=/dev/disk/by-uuid/<volume serial>` +
      `LIVEMEDIA_OFFSET=<bytes>` (casper reads the offset from the environment, which the kernel fills from
      unknown `NAME=value` arguments). Limits: casper-based (Ubuntu-family) ISOs only so far, and the ISO file
      must be stored contiguously; the loader says so if it is fragmented (copy it again to an emptier stick).
      Untested in the loader: that fragmentation message (the extent code itself is tested).
- [x] **Installer** (`toastrack install|update|verify|list`, crate `toastrack-installer`): writes the layout above to\n      a block device or an image file; refuses partitions, mounted disks and non-USB disks without\n      `--allow-internal`; validates the bundle (shim Microsoft-signed, loader signed with the shipped\n      certificate and carrying `.sbat`). Checked with sgdisk, fsck.fat, mtools and a QEMU Secure Boot boot\n      (`scripts/e2e-secboot.sh`).\n- [ ] Installer gaps: Windows disk access, a GUI, 4Kn drives, exFAT, building a release bundle with the real\n      release key, MokManager enrollment walkthrough on real hardware.\n- [ ] Not yet done: El Torito fallback for ISOs whose loader is only inside the boot image, DBX
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

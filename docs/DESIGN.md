# Toastrack design

How Toastrack works, why it is built this way, and what was tried and did not work. For using it see
[USER-GUIDE.md](USER-GUIDE.md); for verification status see the README.

## Decisions
- UEFI x86-64 only; no BIOS.
- Secure Boot: a Microsoft-signed **shim** (taken unmodified from a distro) loads Toastrack's loader, which is signed with the
  owner's key. The key is enrolled once per PC as a MOK (Machine Owner Key) through shim's MokManager, as Ventoy does.
  An optional later step is submitting a Toastrack shim for Microsoft signing (shim-review), which would remove the enrollment.
- Menu: a native UEFI application in Rust (`uefi` crate), software-rendered through the Graphics Output Protocol, keyboard driven.
  Pointer input was tried and removed (below).
- The tools are a Rust workspace; the readers (ISO 9660, FAT, exFAT, UDF, WIM) are `no_std` and shared by loader and tools.
- Main payload: mainstream Linux ISOs. Windows ISOs are supported experimentally ([WINDOWS.md](WINDOWS.md)).

## Stick layout (written by `toastrack install`)
GPT, 1 MiB aligned, 512-byte sectors:
1. `TOASTRACK-ESP`: 64 MiB FAT32 (label `TSTRK-ESP`): `EFI/BOOT/BOOTX64.EFI` (the shim), `mmx64.efi` (MokManager), `grubx64.efi`
   (the signed Toastrack loader, shim's default second stage), `ENROLL_THIS_KEY_IN_MOKMANAGER.cer` (DER).
2. `TOASTRACK`: the rest of the disk, **exFAT** by default (FAT32 with `--data-fs fat32`), label `TOASTRACK`; the loader finds
   its data by that label. Holds `isos/`, anything the user copies, and for Windows ISOs `autounattend.xml` and `toastrack/`.
Volume serial numbers are random per stick.

## The loader
1. **Data volume.** FAT is read through the firmware. The firmware has no exFAT driver, so `core::exfat` (read-only, FAT chains
   with run coalescing, contiguity detection) reads the partition over raw BlockIO.
2. **Menu.** Lists folders and ISOs, computes each ISO's Secure Boot badge on the stick, opens an ISO to list its boot entries.
3. **Virtual optical disk** (`vdisk.rs`). The chosen ISO is exposed as a BlockIo device with a device path, on which
   `connect_controller` makes the firmware's ISO 9660/UDF/El Torito drivers build filesystems. A byte-range *patch overlay*
   serves modified bytes without touching the ISO file (used for the rewritten `grub.cfg` and the directory record size).
4. **Boot paths**
   - *Secure Boot on:* chainload the ISO's own `EFI/BOOT/BOOTX64.EFI` (a Microsoft-signed shim for Ubuntu/Fedora/Debian) from the
     virtual disk; the overlay serves a `boot/grub/grub.cfg` that holds only the entry chosen in the menu with the ISO location
     parameters filled in. The distro's signed GRUB verifies the kernel exactly as on a normal install medium.
   - *Secure Boot off:* load kernel and initrd from the ISO, hand the initrd to the Linux EFI stub through the
     `LINUX_EFI_INITRD_MEDIA_GUID` LoadFile2 protocol, start the kernel with the profile's command line.
   - *Windows:* chainload `bootmgfw.efi` from the El Torito EFI image and publish the hand-off variable (see WINDOWS.md).

### What did not work, and why the above
- **Starting a distro kernel from the loader under Secure Boot.** Firmware `LoadImage` rejects it (the distro key lives in shim,
  not in the firmware db). Asking shim to vouch via `SHIM_LOCK.Verify()` returned `EFI_UNSUPPORTED` for every call from the loader,
  even for Microsoft- and Canonical-signed files, before or after `LoadImage`, with or without MOK/db enrollment; the cause was not
  found (GRUB's calls to the same function work). That code was removed; the overlay approach replaced it.
- **Shim refused the loader until it had an `.sbat` section**; the loader carries one.
- **Pointer input.** The menu once had mouse support. On the test machines (VirtualBox and a Dell laptop) the firmware's console
  splitter exposed a pointer that never delivered events, and no pointer driver was started for USB mice. Polling every pointer
  handle, connecting all drivers, and reading USB HID mice and tablets directly through `EFI_USB_IO` (report-descriptor parsing,
  working in QEMU) still delivered nothing on those machines, so mouse input was dropped in 0.1.0 rather than shipped half-working.
  The USB HID approach is in the git history (`7e76dc1`).

## Secure Boot badge (`core::verdict`)
Per ISO the loader finds `EFI/BOOT/BOOTX64.EFI` (also inside the El Torito EFI image, a FAT image, for ISOs like Windows that keep
it only there), parses the PE Authenticode signature (`core::pe`) and classifies the signer chain:
`Trusted` (Microsoft UEFI CA or Windows CA in the chain), `NeedsEnrollment`, `Unsigned`, `NoUefiLoader`. A Windows boot manager
is recognised by the exact *Windows* signer names (`CN=Microsoft Windows`, Windows Production PCA 2011, Windows UEFI CA 2023);
shim leaves are named "Microsoft Windows UEFI Driver Publisher" and must not match. The badge cannot know a PC's dbx
(revocation) contents. WIN_CERTIFICATE padding must be trimmed before DER parsing (a real Ubuntu ISO exposed this).

## Distro profiles (`core::profile`)
The firmware's view of the ISO disappears when the kernel starts, so the initramfs must find the ISO again. A profile detects the
family from the boot entry, removes location parameters the entry already carries, expands GRUB loopback variables
(`${iso_path}`, `$imgdevpath`, ...; unknown ones become empty as in GRUB) and adds the right ones.

| Family | FAT32 stick | exFAT stick | Verification |
|---|---|---|---|
| casper: Ubuntu, Mint, Pop!_OS | `iso-scan/filename=PATH` | `live-media=/dev/disk/by-uuid/UUID LIVEMEDIA_OFFSET=N` | Booted with a real Ubuntu 24.04 ISO, Secure Boot on and off, FAT32 and exFAT |
| live-boot: Debian live, Kali | `findiso=PATH` | `live-media=... live-media-offset=N` | Booted with a stand-in ISO (Ubuntu userland + live-boot); parameters checked in live-boot's source |
| dracut live: Fedora, RHEL-likes | `iso-scan/filename=PATH` (keeps `root=live:CDLABEL=...`) | same; needs exFAT in the initramfs | Booted with a stand-in ISO built by dracut; checked in dracut's source |
| anaconda installers | `inst.stage2=hd:UUID=UUID:PATH` | same; needs exFAT in the initramfs | documentation only |
| archiso: Arch, Manjaro, EndeavourOS | `img_dev=/dev/disk/by-uuid/UUID img_loop=PATH` | same; needs exFAT in the initramfs | documentation only |
| other | only the entry's own placeholders are filled; otherwise only chainload is offered | | |

The Linux initramfs cannot mount exFAT, so for exFAT the loader passes the ISO's raw location. casper reads
`LIVEMEDIA_OFFSET` from the environment, which the kernel fills from unknown `NAME=value` arguments. The ISO must therefore be one
contiguous run on the partition; the loader says so if not. The stand-in ISOs come from `scripts/build-test-isos.sh`.

Details: entries keep the `search`/`set root=`/`insmod` lines before `linux` (Fedora-style configs search by label); the rewritten
`grub.cfg` may grow into the last sector of the original's extent, with the size patched in the directory record.

## exFAT
`core::exfat` is read-only and verified against images made by exfatprogs' `mkfs.exfat` and written by exfat-fuse (nested folders,
unicode names, 300-entry directories, fragmented files, 4 KiB and 32 KiB clusters; a 4.9 GB contiguous file was also spot-checked by hand). The installer's own
formatter (`installer/exfat_format.rs`) is checked with `fsck.exfat`. Lesson from real use: a damaged copy of a 6.5 GB ISO read as
garbage at the boot loader's offset and showed as `NO UEFI LOADER`; the reader was fine. That is why `C` (checksum) exists.

## Integrity check (`core::checksum`)
Streams the file in 1 MiB chunks through software SHA-256 (the UEFI target has no SSE) and compares with GNU (`hash *name`,
`hash  name`) or BSD (`SHA256 (name) = hash`) formats, matching names case-insensitively and ignoring directories. The result
is shown as a tag and, for damaged copies, blocks the first Enter.

## Installer (`crates/installer`)
Writes the layout to a block device or an image file: GPT, FAT32 for the ESP (the `fatfs` crate), its own exFAT formatter for the
data partition, then the bundle. Refuses partitions, mounted disks and non-USB disks without `--allow-internal`; validates the bundle
(shim Microsoft-signed, loader signed with the shipped certificate, `.sbat` present). Checked with `sgdisk`, `fsck.fat`, `mtools`,
`fsck.exfat` and QEMU boots. Device access: Linux block devices and image files directly; Windows physical drives through `winraw.rs` (volumes locked and
dismounted, old partition table dropped, whole-sector I/O via `sectorio.rs`, partition table re-read afterwards); `target.rs` picks the
right one and `ops.rs` is the shared install/update/verify used by the CLI and by the GUI (`crates/gui`, egui). The system disk is never
offered. Not done: macOS, 4Kn drives, an exFAT writer for `update`.

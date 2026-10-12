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

## Distro profiles (`toastrack-core::profile`)
The firmware's view of the ISO disappears when the kernel starts, so the initramfs must find the ISO file again.
Each family has its own kernel parameters for that. A profile detects the family from the boot entry (kernel path
and arguments), removes the location parameters the entry already carries, expands loopback-config variables
(`${iso_path}`, `$imgdevpath`, ...; unknown ones become empty, as in GRUB) and adds the right parameters:

| Family (examples) | FAT32 stick | exFAT stick | How far it is verified |
|---|---|---|---|
| casper: Ubuntu, Mint, Pop!_OS | `iso-scan/filename=PATH` | `live-media=/dev/disk/by-uuid/UUID LIVEMEDIA_OFFSET=N` (raw; ISO must be contiguous) | **Booted** with a real Ubuntu 24.04 server ISO: Secure Boot on and off, FAT32 and exFAT |
| live-boot: Debian live, Kali | `findiso=PATH` | `live-media=... live-media-offset=N` (raw; contiguous) | **Booted** under Secure Boot on FAT32 and exFAT with a *stand-in* ISO (Ubuntu userland + the same live-boot code). Parameters also checked in live-boot's source. No official Debian/Kali ISO tried. |
| dracut live: Fedora, RHEL-likes | `iso-scan/filename=PATH` (keeps `root=live:CDLABEL=...`) | same; needs exFAT in the ISO's initramfs | **Booted** under Secure Boot on FAT32 and exFAT with a *stand-in* ISO built by real dracut. Parameters also checked in dracut's source. No official Fedora ISO tried. |
| anaconda: Fedora/Alma/Rocky installers | `inst.stage2=hd:UUID=UUID:PATH` | same; needs exFAT in the initramfs | Documentation only, **not booted** |
| archiso: Arch, Manjaro, EndeavourOS | `img_dev=/dev/disk/by-uuid/UUID img_loop=PATH` | same; needs exFAT in the initramfs | Documentation only, **not booted**. These ISOs are usually not Secure Boot signed anyway (the badge shows it). |
| anything else | only the entry's own placeholders are filled in; otherwise "no profile" and only chainload is offered | | |

The stand-in ISOs come from `scripts/build-test-isos.sh` (Ubuntu's archive is the only distro mirror reachable from
the development sandbox, so Debian/Fedora/Arch ISOs could not be downloaded). `toastrack explain <iso>
[--dump-config]` prints the family, status and exact kernel command line for every boot entry of any ISO, so profiles
can be checked against real ISOs without booting; `--dump-config` prints the config the entries came from.

Other details that came out of this work:
- Entries keep the `search`/`set root=`/`insmod` lines that precede `linux`, otherwise GRUB would resolve the kernel
  path on the wrong device (Fedora-style configs search by label).
- The rewritten `grub.cfg` may be longer than the ISO's original: it can grow into the last sector of the original
  file's extent, with the size in the ISO directory record patched in the virtual disk (tested with a 239-byte config).
- The Secure Boot check also looks inside the El Torito EFI image (a FAT image: new read-only FAT12/16/32 reader),
  for ISOs whose loader is not in the file tree.

## Windows ISOs (status: analysis only, nothing booted)
No Windows media could be obtained or run during development (Microsoft hosts are blocked from the sandbox, and
WinPE/boot.wim are Microsoft binaries), so **nothing here has been boot-tested** and no Windows installer is claimed to work.

First real-media check (a Windows 11 25H2 ISO, via `windows-report`): the UDF reader parsed it, the boot manager signers
were `CN=Microsoft Windows` + Windows Production PCA 2011 (so the detection is right and Secure Boot trusts it), the ISO 9660 side
is a one-file `README.TXT` stub, `boot.wim` is 593 MB LZX with 32 KiB chunks and two images (Setup, build 26100, boots),
`install.wim` is 6.87 GB with 11 editions (over 4 GiB: needs exFAT).

What is in place and unit-tested (against images made by genisoimage `-udf` and wimlib, not against real Windows media):
- The Secure Boot badge recognises the Windows boot manager (signed by the *Windows* PCA, not the UEFI CA) as
  trusted; before, a real Windows ISO would have been labelled "needs key enrollment". The menu tags such ISOs `WIN`.
- Windows ISOs keep their loader only in the El Torito EFI image (`efisys.bin`), which the new FAT/El Torito code reads.
- A **WIM reader** (`toastrack-core::wim`, `::lzx`): resource table, chunked LZX decompression (32 KiB to 2 MiB windows),
  image directory trees, hash-verified streams. Verified byte-for-byte against wimlib-made WIMs. Bug found by that test: in the
  length-table coding, pretree code 19 gives a whole run the *same* new length computed from the first entry's previous length;
  applying the delta per entry only goes wrong from the second block of a chunk on.
- A read-only **UDF** reader and a **WIM** header/XML reader (`toastrack-core::udf`, `::wim`), and `toastrack windows-report <iso>`,
  which prints the facts needed to design the fix (see docs/WINDOWS-TESTING.md). UDF: physical partition only; UDF 2.50
  metadata/virtual/sparable partition maps are reported as unsupported.
- Lesson from testing: Microsoft's *UEFI CA* leaf for third-party shims is named "Microsoft Windows UEFI Driver Publisher"; a
  loose substring check mistook Ubuntu's shim for the Windows boot manager. The check now requires the real Windows signers.

What is known to be a problem, from how Windows boots:
1. **Windows Setup cannot find its install files.** The virtual disk lets the firmware load `bootmgfw.efi` and
   `boot.wim` (a RAM disk), but once WinPE runs, the virtual disk no longer exists. Setup then looks for
   `sources\install.wim` on real drives and finds nothing. Windows PE / rescue ISOs (everything inside `boot.wim`) do not have
   this problem and should start.
2. **UDF.** Windows ISOs are UDF; the ISO 9660 side is a stub, so the file tree (`sources/`, install.wim size) is not visible
   to the ISO 9660 reader. Booting only needs El Torito (firmware does that); listing contents would need a UDF reader.
3. **Secure Boot revocations.** Media signed with the 2011 Windows PCA may be refused by PCs that already applied the
   boot-manager revocation updates (BlackLotus mitigations); newer media is signed with "Windows UEFI CA 2023", which older
   firmware does not trust yet. The badge cannot predict this per machine.

Approaches for Windows Setup (approach D supersedes A; all need a Windows ISO and a Windows-capable test setup to verify):
- **A. Inject a locator into WinPE** (Ventoy's way): patch the RAM-loaded `boot.wim` so WinPE runs a small program that finds
  the ISO file on the stick (Windows reads exFAT/FAT32 natively), attaches it via the virtual-disk API and starts
  `setup.exe` from it. Needs a WIM reader/patcher (compressed metadata) plus a Windows helper executable. Largest, keeps the
  one-ISO-file model, works with Secure Boot because Microsoft's boot manager stays unmodified.
- **B. Extract the ISO's files to a folder-less volume** (Rufus' way): Setup then sees `sources\install.wim` on the stick. Needs
  FAT32 with split WIMs, or a filesystem driver shim (UEFI:NTFS style) for exFAT/NTFS; breaks the "drop ISO files in" model.
- **D. Answer file + attach helper (implemented, untested on real hardware)**: instead of patching `boot.wim`, the stick root
  holds `autounattend.xml` (windowsPE pass, RunSynchronous) that runs `toastrack\attach.exe` (`crates/winattach`, `no_std`,
  imports only kernel32/advapi32/msvcrt/virtdisk, all present in the Setup image per windows-report). The loader publishes
  `ToastrackIso` (volume serial + path, `core::winreq`); the helper finds the volume and attaches the ISO read-only. Result (VirtualBox, UEFI, Secure Boot off, Win11 25H2): Setup reached the language page and then the disk-selection page,
  i.e. it found `install.wim`, so the answer file, volume visibility and variable read all worked. Still untested: Secure Boot on,
  real hardware, a completed install, attach.log contents. Originally unverified:
  Setup searching removable-media roots for the answer file, the USB volume being visible, the variable being readable from WinPE.
- **C. Windows PE / rescue ISOs only** (what exists now): chainload works in principle; Windows Setup is declined with a message.

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

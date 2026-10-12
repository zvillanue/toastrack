# Toastrack

A multiboot USB stick for **UEFI PCs, with Secure Boot support**. Copy ISO files onto the stick, pick one from a graphical
menu, and boot it. Like Ventoy, but the boot chain is a Microsoft-signed shim plus your own enrolled key, so it works with
Secure Boot **on**, and the menu tells you per ISO whether it will boot that way.

**Version 0.1.0 - first release, expect rough edges.** Read [What works and what is verified](#what-works-and-what-is-verified)
before relying on it.

## Features
- Graphical menu (native UEFI app written in Rust): browse folders and subfolders, keyboard control.
- **Secure Boot**: firmware -> Microsoft-signed shim -> Toastrack (signed with your key, enrolled once per PC) -> the ISO's own
  signed bootloader. No Secure Boot switching off needed for mainstream Linux ISOs.
- **Per-ISO badge**: `SECURE BOOT OK`, `NEEDS KEY ENROLL`, `UNSIGNED` or `NO UEFI LOADER`, computed on the stick by reading the
  ISO's boot loader signature. A Windows ISO is tagged `WIN`.
- **Files over 4 GB**: the ISO partition is exFAT by default, read by Toastrack's own driver (the firmware has none).
- **Integrity check**: press `C` on an ISO to verify its SHA-256 against the `SHA256SUMS` file next to it; damaged copies are
  flagged `DAMAGED` before you waste time booting them.
- Distro profiles that tell each family's initramfs where the ISO lives: Ubuntu family, Debian live, Fedora/RHEL live,
  Fedora-style installers, Arch family.
- **Windows 10/11 ISOs (experimental)**: Setup starts and finds its install files through a small helper that attaches the ISO as a
  virtual DVD. See [docs/WINDOWS.md](docs/WINDOWS.md).
- Installer and diagnostics in one command line tool, `toastrack`. UEFI x86-64 only; no BIOS.

## Quick start
### From a release package (Windows or Linux)
Download `toastrack-<version>-windows-x86_64.zip` or `-linux-x86_64.tar.gz` from the releases page, unpack it completely
(the `bundle` folder must stay next to the programs), plug in the stick (**everything on it will be erased**) and run
**`toastrack-gui`**: choose the stick, press *Write stick...*. Windows asks for administrator rights; on Linux the app offers to
restart itself with them (polkit) or run it with `sudo`. Verify downloads as described in the release notes
(`SHA256SUMS` + `SHA256SUMS.sig`; the signing key's fingerprint is shown in the app and in `bundle/VERSION.txt`).
*(No signed release has been published yet; until then build from source, below.)*

### From source (Linux, Secure Boot stick)
You need a Rust toolchain ([rustup](https://rustup.rs)), the UEFI target and a few tools. On Arch:
`sudo pacman -S rustup sbsigntools openssl xorriso git && rustup default stable && rustup target add x86_64-unknown-uefi`
(Debian/Ubuntu: `sbsigntool openssl xorriso git`).

```sh
git clone https://github.com/zvillanue/toastrack && cd toastrack
cargo build --release -p toastrack -p toastrack-gui
cargo build --release -p toastrack-loader --target x86_64-unknown-uefi

# Secure Boot stick: needs the shim from any Ubuntu ISO (Microsoft-signed) and makes you a signing key
scripts/make-dev-bundle.sh ~/tr-bundle /path/to/any-ubuntu.iso target/x86_64-unknown-uefi/release/toastrack-loader.efi

target/release/toastrack list                                         # find your USB stick
sudo target/release/toastrack install /dev/sdX --bundle ~/tr-bundle   # ERASES /dev/sdX   (or: sudo target/release/toastrack-gui)
```
On Windows from source: `cargo build --release -p toastrack -p toastrack-gui` and run as administrator; disks are
`\\.\PhysicalDriveN` (see `toastrack list`).

Then mount the **TOASTRACK** partition, copy ISOs into its `isos/` folder (subfolders are fine), run `sync` (Linux) or eject
safely (Windows), and boot the stick (boot menu key: usually F12, F9 or Esc).

**First boot with Secure Boot on, once per PC:** shim shows a blue *MOK management* screen. Choose *Enroll key from disk* ->
`ENROLL_THIS_KEY_IN_MOKMANAGER.cer` -> confirm -> reboot. Compare the fingerprint MokManager shows with the one the app printed.
With the development bundle, keep `~/tr-bundle`: it holds your signing key; reusing it means PCs you already enrolled keep
working when you update the stick.

For a PC with Secure Boot **off**, no shim or key is needed: `toastrack install /dev/sdX --unsigned-loader <loader.efi>`.

The full walkthrough is in [docs/USER-GUIDE.md](docs/USER-GUIDE.md).

## In the menu
| Key | Action |
|---|---|
| Up / Down, PgUp / PgDn, Home / End | move |
| Enter | open a folder, open an ISO's boot entries, boot an entry |
| Esc | back (leaves the menu at the top) |
| `C` | check the selected ISO's SHA-256 against its `.sha256` / `SHA256SUMS` file (Esc cancels) |

## What works and what is verified
"Booted" means the ISO was started and its system came up. Development had no distro mirror access except Ubuntu's, so some
families are verified with stand-in ISOs.

| Area | Status |
|---|---|
| Ubuntu (casper) ISO, Secure Boot on and off, FAT32 and exFAT | **Booted** in QEMU/OVMF with a real Ubuntu 24.04 server ISO |
| Secure Boot chain with a locally signed loader and key enrollment | **Verified** in QEMU/OVMF with Microsoft keys, and the menu runs under Secure Boot in VirtualBox; shim refuses an unsigned or un-enrolled loader as it should |
| Debian live and Fedora/RHEL live profiles | **Booted** with stand-in ISOs built from the same boot code (live-boot, dracut); no official Debian/Fedora ISO tried |
| Fedora-style installers (anaconda), Arch family | Parameters follow the distros' documentation; **not booted** |
| Windows 11 25H2 ISO | In VirtualBox (UEFI, Secure Boot **off**): boot manager -> Setup -> install media found -> disk selection. No install completed; Secure Boot on and real hardware not tried |
| exFAT reader/formatter | Checked against exfatprogs and exfat-fuse images and `fsck.exfat` |
| Real PCs | Menu displayed on a Dell laptop; a Framework laptop correctly refused an unsigned stick under Secure Boot. **A complete Linux boot on real hardware is not confirmed yet** |

Known limitations in 0.1.0:
- **No mouse.** Mouse input was attempted (relative, absolute and direct USB HID) but firmware on the test machines never
  delivered pointer events, so it was removed. The menu is keyboard-driven.
- The stick maker runs on **Linux and Windows** (command line and a graphical app). The Windows disk-writing code has been
  built and tested with simulated whole-sector I/O, but **not yet on a real Windows PC**. No macOS version.
- Until the first signed release is published you build your own bundle with `scripts/make-dev-bundle.sh` (your own key, shim
  taken from an Ubuntu ISO). The release pipeline (CI build, offline signing, packages) exists but has not been run on GitHub yet; see [docs/RELEASING.md](docs/RELEASING.md).
- On an exFAT stick the ISO file must be stored in one piece (contiguous) for Ubuntu/Debian/Fedora-style initramfs to find
  it (they cannot read exFAT). Toastrack says so if it is fragmented; copy the ISO again onto an emptier stick.
- Booting under Secure Boot goes through the ISO's own signed GRUB with a rewritten, single-entry `grub.cfg` served from the
  virtual disk (the ISO file is not modified). ISOs without a signed GRUB cannot be booted with Secure Boot on.
- The Secure Boot badge cannot know a PC's revocation list (dbx), so it can say OK for a loader a particular PC refuses.
- `toastrack update` replaces the boot partition only; it does not touch the data partition.

## Command line
```
toastrack list                                   disks that could be a stick (Linux and Windows)
toastrack install  <disk|image> --bundle DIR     erase and set up a stick (--data-fs fat32, --unsigned-loader, --attach-exe)
toastrack update   <disk|image> --bundle DIR     replace shim/loader/key, keep the ISOs
toastrack verify   <disk|image> --bundle DIR     read the stick back and compare with the bundle
toastrack check    iso...                        Secure Boot verdict and boot entries
toastrack explain  iso                           distro profile and exact kernel command line per entry
toastrack check-volume <partition> <path>        diagnose an ISO through the loader's own exFAT reader
toastrack windows-report iso                     describe a Windows ISO (safe to paste into an issue)
```

## Documentation
- [User guide](docs/USER-GUIDE.md) - installing, adding ISOs, Secure Boot enrollment, updating, troubleshooting
- [Windows ISOs](docs/WINDOWS.md) - how Setup is handled, how to test, what is unproven
- [Design](docs/DESIGN.md) - how it works, decisions, what was tried and did not work
- [Development](docs/DEVELOPMENT.md) - building, tests, test scripts
- [Releasing](docs/RELEASING.md) - signing key custody, building and publishing a release
- [Roadmap](docs/ROADMAP.md) and [Changelog](CHANGELOG.md)

## Safety
`toastrack install` **erases the whole target device**. It refuses partitions, mounted disks and non-USB disks unless you pass
`--allow-internal`, and asks you to type the device name. Check the device with `toastrack list` first.

## License
MIT, see [LICENSE](LICENSE). Toastrack does not include a shim; `make-dev-bundle.sh` copies the unmodified, Microsoft-signed
shim from a distro ISO you supply, as distributions do, and its signature is what makes the Secure Boot chain work.

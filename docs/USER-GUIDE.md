# Toastrack user guide

## 1. What you need
- A USB stick (8 GB or more; the data partition takes everything after a 64 MiB boot partition) and a Linux PC to prepare it.
- A Rust toolchain with the UEFI target: `rustup target add x86_64-unknown-uefi`.
- For a Secure Boot stick: `sbsigntools` (Debian/Ubuntu: `sbsigntool`), `openssl`, `xorriso`, and any Ubuntu ISO (only its
  Microsoft-signed shim is used).
- For Windows ISO support (optional): `mingw-w64-gcc` and `rustup target add x86_64-pc-windows-gnu`.

Get the code and build:

```sh
git clone https://github.com/zvillanue/toastrack && cd toastrack
cargo build --release -p toastrack
cargo build --release -p toastrack-loader --target x86_64-unknown-uefi
# optional, Windows ISOs:
cargo build --release -p toastrack-winattach --target x86_64-pc-windows-gnu
```
Outputs: `target/release/toastrack`, `target/x86_64-unknown-uefi/release/toastrack-loader.efi`,
`target/x86_64-pc-windows-gnu/release/toastrack-attach.exe`.

## 2. Make a bundle (Secure Boot sticks)
A *bundle* is the folder `toastrack install` copies onto the boot partition:

| File | What it is |
|---|---|
| `shimx64.efi` | Microsoft-signed shim (from a distro; unmodified) |
| `mmx64.efi` | MokManager, the key enrollment screen |
| `grubx64.efi` | the Toastrack loader, **signed with your key** (shim's default second stage) |
| `ENROLL_THIS_KEY_IN_MOKMANAGER.cer` | your public key, to enroll on each PC |
| `toastrack-attach.exe` | Windows helper (optional) |

`scripts/make-dev-bundle.sh <bundle-dir> <ubuntu.iso> [loader.efi]` creates all of it: it takes the shim and MokManager out
of the Ubuntu ISO, creates a signing key in `<bundle-dir>` if there is none, signs the loader and adds the Windows helper if it
builds. **Keep the folder**: re-running with the same folder re-uses the key, so PCs you already enrolled keep trusting the
stick after you rebuild the loader. Losing the key means enrolling the new one on every PC again. Never share `key.pem`.

```sh
scripts/make-dev-bundle.sh ~/tr-bundle ubuntu.iso target/x86_64-unknown-uefi/release/toastrack-loader.efi
```
The key made by the script is valid for a year; make a new bundle when it expires.

## 3. Write the stick
```sh
target/release/toastrack list                              # shows disks, marks USB ones, [MOUNTED] ones are refused
sudo target/release/toastrack install /dev/sdX --bundle ~/tr-bundle
```
- **This erases `/dev/sdX`.** The tool asks you to type the device name. Unmount the stick's partitions first if it complains.
- The data partition is exFAT (files over 4 GB). `--data-fs fat32` gives FAT32 instead (4 GB file limit, but readable by
  every OS and firmware).
- You can also install into an image file to try in a VM: `toastrack install stick.img --size 8G --bundle ~/tr-bundle`.
- Check it: `sudo toastrack verify /dev/sdX --bundle ~/tr-bundle` reads the stick back and compares it with the bundle.
- **No Secure Boot** (the PC has it off): skip the bundle, use `--unsigned-loader target/.../toastrack-loader.efi`
  (add `--attach-exe ...toastrack-attach.exe` for Windows ISOs). Such a stick is refused by a PC with Secure Boot on
  ("Verification failed: (0x1A) Security Violation").

## 4. Add ISOs
Mount the partition labelled **TOASTRACK**, copy ISO files into `isos/` (any folder structure below it works, or anywhere
on the partition), then:

```sh
sync          # wait until it returns, the stick may still be writing after cp finishes
umount /run/media/you/TOASTRACK
```
Unplugging before `sync` finishes damages files, which then boot badly or not at all. Put the checksum file the distro
publishes (`SHA256SUMS` or `<name>.iso.sha256`) in the same folder if you want the menu to verify the copy.

## 5. Boot
1. Boot the stick (boot menu key: F12, F9, Esc or F2 depending on the PC; the entry is usually named after the stick).
2. **Secure Boot on, first time on this PC:** shim shows a blue *Perform MOK management* screen. Choose *Enroll key from
   disk* (or *Enroll MOK*), pick `ENROLL_THIS_KEY_IN_MOKMANAGER.cer`, *Continue*, *Yes*, reboot. This is done once per PC (the key lives in that PC's firmware).
3. The Toastrack menu appears. The top right shows `Secure Boot: ON/OFF`, the right edge the data partition type.
4. Navigate (arrows, Enter, Esc). Select an ISO and press Enter to see its boot entries, Enter again to boot one.
   - Some ISOs offer *Boot the ISO's own bootloader* (chainload) in addition to their menu entries.
   - With Secure Boot on, an ISO tagged `UNSIGNED` or `NO UEFI LOADER` is blocked with a message instead of failing later.

### Badges
| Badge | Meaning |
|---|---|
| `SECURE BOOT OK` | the ISO's loader is signed through Microsoft's chain: boots with Secure Boot on |
| `NEEDS KEY ENROLL` | signed, but with a key that is not in your PC's firmware: boots only if that key is enrolled |
| `UNSIGNED` | no signature: only with Secure Boot off |
| `NO UEFI LOADER` | no x86-64 UEFI boot loader found (BIOS-only, ARM, or unreadable: see Troubleshooting) |
| `UNREADABLE` | the file could not be read as an ISO |
| `SUM OK` / `DAMAGED` | result of pressing `C` (checksum check) |
| `WIN` (tag, left) | a Windows ISO |

### Checking an ISO (`C`)
Select the ISO and press `C`. Toastrack reads the whole file and computes its SHA-256 (a progress line shows; Esc cancels).
It then looks in the same folder for `<name>.sha256`, `<name>.sha256sum`, `SHA256SUMS`, `sha256sums`, `SHA256SUM`,
`sha256sum.txt` or `SHA256SUMS.txt`. Result: green `SUM OK`; red `DAMAGED` (with both hashes; Enter then asks for confirmation before booting
it); or the hash plus the reason no comparison was possible (no checksum file, or the file lists other names).

## 6. Updating and maintaining
- **New Toastrack version, same key:** rebuild the loader, `scripts/make-dev-bundle.sh ~/tr-bundle ubuntu.iso <loader.efi>`,
  then `sudo toastrack update /dev/sdX --bundle ~/tr-bundle`. ISOs stay; no re-enrollment.
- `update` only rewrites the boot partition. To change the data partition filesystem, run `install` again.
- Add or delete ISOs by simply copying and deleting files; nothing needs regenerating.

## 7. Windows ISOs (experimental)
Needs the Windows helper on the stick (build it, see section 1; `make-dev-bundle.sh` adds it). Copy the Windows 10/11 ISO
into `isos/`, boot it from the menu, and Setup starts. Details, limits and how to report results: [WINDOWS.md](WINDOWS.md).

## 8. Troubleshooting
| Symptom | Likely cause and fix |
|---|---|
| "Verification failed: (0x1A) Security Violation" before any menu | The stick has no shim (made with `--unsigned-loader`) but the PC has Secure Boot on. Use a bundle stick, or turn Secure Boot off. |
| Blue MokManager screen every boot | The key was not enrolled, or the bundle was rebuilt with a new key. Enroll `ENROLL_THIS_KEY_...cer` again. |
| ISO shows `NO UEFI LOADER` although the distro supports UEFI | Usually a damaged copy (the loader file reads as garbage). Run `toastrack check iso` on the original, then `sha256sum` both copies; copy again, `sync`, unmount, re-check with `C`. `sudo toastrack check-volume /dev/sdX2 isos/name.iso` shows what the menu sees. |
| Boots to a prompt "Could not find the ISO" | exFAT stick and the ISO is fragmented, or the distro's initramfs cannot use the raw location. Copy the ISO again onto an empty stick (one run), or use `--data-fs fat32` for ISOs under 4 GB. `toastrack explain iso` shows the command line used. |
| `sync` takes minutes | Cheap sticks write slowly. Watch `grep -E 'Dirty\|Writeback' /proc/meminfo`; check `dmesg` for I/O errors. Do not unplug until it returns. |
| `install`: "Permission denied" | Run the compiled binary with `sudo` (not `sudo cargo run`). |
| Menu is blank or the screen stays black | Firmware without a graphics output for the stick; try another boot entry for the same stick (UEFI vs legacy) and a different port. |
| ISO with a rare layout shows no boot entries | `toastrack explain iso --dump-config` and open an issue with the output. |

Reporting a problem: include `toastrack check`/`explain` output for the ISO, the PC model, and whether Secure Boot was on.

# toastrack

Multiboot USB stick for UEFI PCs with **Secure Boot** support: copy ISO files onto the stick, pick one from a
graphical menu (folders, mouse, per-ISO Secure Boot badge) and boot it. Status: early development - see
[docs/DESIGN.md](docs/DESIGN.md) for what works and what is verified.

Boots Linux ISOs of these families, tested as shown in [docs/DESIGN.md](docs/DESIGN.md): Ubuntu family (tested with a real
ISO), Debian live and Fedora/RHEL live (tested with stand-in ISOs built from the same boot code), Fedora-style installers and
Arch family (follow the distros' documentation, not yet booted). Windows ISOs are not supported yet.

```
toastrack check ubuntu.iso                      # will it boot with Secure Boot on? which boot entries?
toastrack explain some.iso                     # which distro profile, and the exact kernel command line per entry
toastrack list                                  # disks
toastrack install /dev/sdX --bundle ./bundle    # ERASES the disk and sets it up (asks for confirmation)
toastrack update  /dev/sdX --bundle ./bundle    # replace boot files, keep the ISOs
toastrack verify  /dev/sdX --bundle ./bundle
```
Then copy ISOs into the `TOASTRACK` partition (exFAT by default, so files over 4 GB work; `--data-fs fat32` if you prefer). On the first boot of each PC with Secure Boot on, shim shows
MokManager: *Enroll key from disk* -> `ENROLL_THIS_KEY_IN_MOKMANAGER.cer` -> reboot.

Development: `cargo test`, `scripts/make-dev-bundle.sh`, `scripts/e2e-secboot.sh` (QEMU).

## Checking ISO integrity
In the menu, select an ISO and press `C`. Toastrack hashes the file (SHA-256, Esc cancels) and compares it with the checksum
published next to it: `<name>.sha256`, or a `SHA256SUMS` / `sha256sum.txt` file in the same folder (the files distros
provide). A matching copy gets a green `SUM OK` tag, a mismatch a red `DAMAGED` tag and a warning before it will boot; with no
checksum file the hash is shown so it can be compared by hand.

## Mouse troubleshooting
The menu polls every pointing device the firmware offers (relative mice and absolute tablets). If the pointer is drawn but does
not respond, press `F2`: the bottom line then shows how many pointer devices the firmware exposes and the raw events received
(`events rel=0 abs=0` while moving the mouse means the firmware is not delivering mouse input to UEFI programs; the keyboard
always works). Include that line when reporting a mouse problem.
